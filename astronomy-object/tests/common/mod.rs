//! Shared helpers for the `astronomy-object` integration tests.
//!
//! Tests build Astronomy IR in-memory and lower it with the backend. The
//! execution helpers emit an ELF object with the **built-in encoder**, link
//! it against a tiny C driver with `gcc` and run it. When `nasm` is also
//! installed, the NASM text is assembled too and its `.text`, `.rodata` and
//! relocations must match the built-in object exactly — NASM serves as an
//! independent reference for the encoder, never as part of the pipeline.
//! Without `gcc` the execution helpers return `None` and the test prints a
//! skip message instead of failing.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use astronomy::{Abi, FunctionBuilder, Linkage, Module, ModuleBuilder, TypeId, Verifier};

/// Verifies and lowers a module, returning the NASM text.
pub fn compile_ok(module: Module) -> String {
    let verified = Verifier::verify(module).expect("module must verify");
    astronomy_object::compile_nasm(&verified).expect("module must lower to NASM")
}

/// Verifies then lowers, expecting a backend error.
pub fn compile_err(module: Module) -> astronomy_object::BackendError {
    let verified = Verifier::verify(module).expect("module must verify");
    astronomy_object::compile_nasm(&verified).expect_err("expected a backend error")
}

/// Builds a module with a single exported `abi=c` function.
pub fn one_function<F>(name: &str, params: &[(&str, TypeId)], result: TypeId, body: F) -> Module
where
    F: FnOnce(&mut FunctionBuilder),
{
    let mut builder = ModuleBuilder::with_name("test");
    let id = builder
        .declare_function(name, Linkage::Exported, Abi::C, params, result)
        .unwrap();
    let mut fb = builder.function_builder(id).unwrap();
    fb.append_block();
    body(&mut fb);
    builder.finish()
}

/// Result of running a compiled program.
pub struct RunOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    /// Trimmed stdout (the usual assertion target).
    pub fn out(&self) -> &str {
        self.stdout.trim()
    }
}

fn tool_works(program: &str, flag: &str) -> bool {
    matches!(Command::new(program).arg(flag).output(), Ok(o) if o.status.success())
}

/// True when `gcc` (the C driver and linker) is present.
pub fn gcc_available() -> bool {
    tool_works("gcc", "--version")
}

/// True when `nasm` (the reference assembler) is present.
pub fn nasm_available() -> bool {
    tool_works("nasm", "-v")
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn temp_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("astronomy-object-{}-{n}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Compiles `module` to an ELF object with the built-in encoder, links it
/// with `driver` C source and runs it.
///
/// Returns `None` when `gcc` is unavailable. When `nasm` is available the
/// object is first cross-checked against NASM's assembly of the same
/// program (see [`assert_matches_nasm`]). Panics with the assembly listing
/// when linking or running fails, so a codegen bug is immediately visible.
pub fn assemble_and_run(module: Module, driver: &str) -> Option<RunOutput> {
    if !gcc_available() {
        eprintln!("skipping execution test: `gcc` not available");
        return None;
    }

    let verified = Verifier::verify(module).expect("module must verify");
    let asm = astronomy_object::compile_nasm(&verified).expect("module must lower to NASM");
    let object = astronomy_object::compile_object(&verified).expect("module must lower to ELF");

    let dir = temp_dir();
    if nasm_available() {
        assert_matches_nasm(&dir, &asm, &object);
    }

    let obj_path = dir.join("module.o");
    std::fs::write(&obj_path, &object).unwrap();
    let c_path = dir.join("driver.c");
    std::fs::write(&c_path, driver).unwrap();
    let driver_obj = dir.join("driver.o");
    let cc = Command::new("gcc")
        .args(["-O0", "-w", "-c"])
        .arg(&c_path)
        .arg("-o")
        .arg(&driver_obj)
        .output()
        .expect("failed to spawn gcc");
    assert!(
        cc.status.success(),
        "driver failed to compile:\n{}",
        String::from_utf8_lossy(&cc.stderr)
    );
    // Default `gcc` output is a PIE on most distributions; the object must
    // link there without `-no-pie`, and without any linker warning (such as
    // the executable-stack warning a missing `.note.GNU-stack` triggers).
    let prog_path = dir.join("prog");
    let link = Command::new("gcc")
        .arg(&driver_obj)
        .arg(&obj_path)
        .arg("-o")
        .arg(&prog_path)
        .output()
        .expect("failed to spawn gcc");
    assert!(
        link.status.success() && link.stderr.is_empty(),
        "linking failed or warned:\n{}\n--- assembly ---\n{asm}",
        String::from_utf8_lossy(&link.stderr)
    );

    let run = Command::new(&prog_path)
        .output()
        .expect("failed to run program");
    let result = RunOutput {
        code: run.status.code(),
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&run.stderr).into_owned(),
    };
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        run.status.success(),
        "compiled program failed: {}\nstdout: {}\nstderr: {}\n--- assembly ---\n{asm}",
        run.status,
        result.stdout,
        result.stderr
    );
    Some(result)
}

/// Assembles `asm` with `nasm -f elf64` and asserts the built-in `object`
/// has byte-identical `.text` and `.rodata` and equivalent relocations.
pub fn assert_matches_nasm(dir: &Path, asm: &str, object: &[u8]) {
    let asm_path = dir.join("module.asm");
    std::fs::write(&asm_path, asm).unwrap();
    let nasm_obj = dir.join("nasm.o");
    let nasm = Command::new("nasm")
        .args(["-f", "elf64"])
        .arg(&asm_path)
        .arg("-o")
        .arg(&nasm_obj)
        .output()
        .expect("failed to spawn nasm");
    assert!(
        nasm.status.success(),
        "nasm failed:\n{}\n--- assembly ---\n{asm}",
        String::from_utf8_lossy(&nasm.stderr)
    );
    let reference = std::fs::read(&nasm_obj).unwrap();
    let (ours, theirs) = (Elf::parse(object), Elf::parse(&reference));
    for section in [".text", ".rodata"] {
        assert!(
            ours.section(section) == theirs.section(section),
            "`{section}` differs from NASM's encoding\n--- assembly ---\n{asm}"
        );
    }
    // NASM uses R_X86_64_PC32 for calls where the built-in encoder uses the
    // PIE-friendly R_X86_64_PLT32; both resolve to `S + A - P`.
    let normalize = |relocs: Vec<Rela>| -> Vec<(u64, String, i64)> {
        relocs
            .into_iter()
            .map(|r| (r.offset, r.symbol, r.addend))
            .collect()
    };
    assert_eq!(
        normalize(ours.relocations()),
        normalize(theirs.relocations()),
        "relocations differ from NASM's\n--- assembly ---\n{asm}"
    );
}

// ---------------------------------------------------------------------------
// Minimal ELF64 reader (just enough to inspect objects in tests)
// ---------------------------------------------------------------------------

/// A parsed ELF64 little-endian relocatable object.
pub struct Elf<'a> {
    pub bytes: &'a [u8],
    pub sections: Vec<ElfSection>,
}

#[derive(Debug, Clone)]
pub struct ElfSection {
    pub name: String,
    pub kind: u32,
    pub flags: u64,
    pub offset: usize,
    pub size: usize,
    pub link: u32,
    pub info: u32,
    pub align: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElfSymbol {
    pub name: String,
    pub bind: u8,
    pub kind: u8,
    pub shndx: u16,
    pub value: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rela {
    pub offset: u64,
    pub kind: u32,
    /// Symbol name, or the section name for `STT_SECTION` symbols.
    pub symbol: String,
    pub addend: i64,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}
fn c_str(b: &[u8], at: usize) -> String {
    let end = b[at..].iter().position(|&c| c == 0).unwrap() + at;
    String::from_utf8(b[at..end].to_vec()).unwrap()
}

impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Elf<'a> {
        assert_eq!(&bytes[..4], b"\x7fELF", "not an ELF file");
        let shoff = u64_at(bytes, 0x28) as usize;
        let shnum = u16_at(bytes, 0x3C) as usize;
        let shstrndx = u16_at(bytes, 0x3E) as usize;
        let raw: Vec<_> = (0..shnum)
            .map(|i| {
                let h = shoff + i * 64;
                (
                    u32_at(bytes, h),
                    ElfSection {
                        name: String::new(),
                        kind: u32_at(bytes, h + 4),
                        flags: u64_at(bytes, h + 8),
                        offset: u64_at(bytes, h + 24) as usize,
                        size: u64_at(bytes, h + 32) as usize,
                        link: u32_at(bytes, h + 40),
                        info: u32_at(bytes, h + 44),
                        align: u64_at(bytes, h + 48),
                    },
                )
            })
            .collect();
        let names_at = raw[shstrndx].1.offset;
        let sections = raw
            .into_iter()
            .map(|(name, mut s)| {
                s.name = c_str(bytes, names_at + name as usize);
                s
            })
            .collect();
        Elf { bytes, sections }
    }

    pub fn find(&self, name: &str) -> Option<(usize, &ElfSection)> {
        self.sections
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
    }

    /// Contents of a section (`None` when absent).
    pub fn section(&self, name: &str) -> Option<&'a [u8]> {
        self.find(name)
            .map(|(_, s)| &self.bytes[s.offset..s.offset + s.size])
    }

    pub fn symbols(&self) -> Vec<ElfSymbol> {
        let (_, symtab) = self.find(".symtab").expect("object has a symbol table");
        let strtab = &self.sections[symtab.link as usize];
        (0..symtab.size / 24)
            .map(|i| {
                let e = symtab.offset + i * 24;
                let info = self.bytes[e + 4];
                let shndx = u16_at(self.bytes, e + 6);
                let mut name = c_str(self.bytes, strtab.offset + u32_at(self.bytes, e) as usize);
                if info & 0xF == 3 {
                    name = self.sections[shndx as usize].name.clone();
                }
                ElfSymbol {
                    name,
                    bind: info >> 4,
                    kind: info & 0xF,
                    shndx,
                    value: u64_at(self.bytes, e + 8),
                    size: u64_at(self.bytes, e + 16),
                }
            })
            .collect()
    }

    /// Relocations of `.rela.text`, in file order.
    pub fn relocations(&self) -> Vec<Rela> {
        let Some((_, rela)) = self.find(".rela.text") else {
            return Vec::new();
        };
        let symbols = self.symbols();
        (0..rela.size / 24)
            .map(|i| {
                let e = rela.offset + i * 24;
                let info = u64_at(self.bytes, e + 8);
                Rela {
                    offset: u64_at(self.bytes, e),
                    kind: info as u32,
                    symbol: symbols[(info >> 32) as usize].name.clone(),
                    addend: u64_at(self.bytes, e + 16) as i64,
                }
            })
            .collect()
    }
}

/// Convenience: run and assert the program printed `expected` (trimmed).
pub fn assert_prints(module: Module, driver: &str, expected: &str) {
    if let Some(out) = assemble_and_run(module, driver) {
        assert_eq!(
            out.out(),
            expected,
            "unexpected program output\nstdout: {}\nstderr: {}",
            out.stdout,
            out.stderr
        );
    }
}
