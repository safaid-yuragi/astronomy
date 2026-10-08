//! Direct ELF64 object emission (`compile_object`): file structure,
//! symbols and relocations, branch relaxation, determinism, and linking as
//! executables and shared libraries — all without an external assembler.

mod common;

use std::process::Command;

use astronomy::{Abi, Function, Linkage, Module, ModuleBuilder, TypeId, Verifier};
use common::{assert_prints, gcc_available, temp_dir, Elf};

const I64: TypeId = TypeId::I64;
const I32: TypeId = TypeId::I32;

const STB_LOCAL: u8 = 0;
const STB_GLOBAL: u8 = 1;
const STT_NOTYPE: u8 = 0;
const STT_FUNC: u8 = 2;
const R_X86_64_PC32: u32 = 2;
const R_X86_64_PLT32: u32 = 4;

fn object(module: Module) -> Vec<u8> {
    let verified = Verifier::verify(module).expect("module must verify");
    astronomy_object::compile_object(&verified).expect("module must lower to ELF")
}

/// `greet()` calls an internal helper and `puts`; `unused` is declared but
/// never called.
fn greeter() -> Module {
    let mut b = ModuleBuilder::with_name("greeter");
    let i8_ptr = b.ptr_type(TypeId::I8);
    let puts = b
        .declare_extern("puts", Abi::C, &[i8_ptr], false, I32)
        .unwrap();
    b.declare_extern("unused", Abi::C, &[], false, I32).unwrap();
    let helper = b
        .declare_function("helper", Linkage::Internal, Abi::C, &[("x", I64)], I64)
        .unwrap();
    {
        let mut fb = b.function_builder(helper).unwrap();
        fb.append_block();
        let x = fb.param(0);
        let doubled = fb.add(x, x).unwrap();
        fb.ret(Some(doubled)).unwrap();
    }
    let greet = b
        .declare_function("greet", Linkage::Exported, Abi::C, &[("x", I64)], I64)
        .unwrap();
    {
        let mut fb = b.function_builder(greet).unwrap();
        fb.append_block();
        let x = fb.param(0);
        let msg = fb.const_string(b"hello from a built-in object").unwrap();
        fb.call(puts, &[msg]).unwrap();
        let r = fb.call(helper, &[x]).unwrap().unwrap();
        fb.ret(Some(r)).unwrap();
    }
    b.finish()
}

#[test]
fn object_has_the_expected_sections() {
    let bytes = object(greeter());
    let elf = Elf::parse(&bytes);
    assert_eq!(&bytes[4..8], &[2, 1, 1, 0], "ELF64, little-endian, SysV");
    assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 1, "ET_REL");
    assert_eq!(u16::from_le_bytes([bytes[18], bytes[19]]), 62, "EM_X86_64");

    let names: Vec<&str> = elf.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "",
            ".text",
            ".rodata",
            ".note.GNU-stack",
            ".rela.text",
            ".symtab",
            ".strtab",
            ".shstrtab"
        ]
    );
    let (_, text) = elf.find(".text").unwrap();
    assert_eq!(text.flags, 0x6, "alloc + exec");
    let (_, rodata) = elf.find(".rodata").unwrap();
    assert_eq!(rodata.flags, 0x2, "alloc only");
    let (_, note) = elf.find(".note.GNU-stack").unwrap();
    assert_eq!(
        (note.flags, note.size),
        (0, 0),
        "non-executable stack marker"
    );
    assert_eq!(
        elf.section(".rodata").unwrap(),
        b"hello from a built-in object\0"
    );
}

#[test]
fn symbols_reflect_linkage() {
    let bytes = object(greeter());
    let elf = Elf::parse(&bytes);
    let symbols = elf.symbols();
    let find = |name: &str| symbols.iter().find(|s| s.name == name).cloned();

    let helper = find("helper").expect("internal function is a local symbol");
    assert_eq!((helper.bind, helper.kind), (STB_LOCAL, STT_FUNC));
    let greet = find("greet").expect("exported function is a global symbol");
    assert_eq!((greet.bind, greet.kind), (STB_GLOBAL, STT_FUNC));
    let puts = find("puts").expect("called declaration is undefined");
    assert_eq!(
        (puts.bind, puts.kind, puts.shndx),
        (STB_GLOBAL, STT_NOTYPE, 0)
    );
    assert!(
        find("unused").is_none(),
        "uncalled declarations are omitted"
    );

    // Function sizes tile `.text` exactly.
    let text_len = elf.section(".text").unwrap().len() as u64;
    assert_eq!(helper.value, 0);
    assert_eq!(greet.value, helper.size);
    assert_eq!(helper.size + greet.size, text_len);

    // Locals precede globals, and `sh_info` points at the first global.
    let first_global = symbols.iter().position(|s| s.bind == STB_GLOBAL).unwrap();
    assert!(symbols[first_global..].iter().all(|s| s.bind == STB_GLOBAL));
    let (_, symtab) = elf.find(".symtab").unwrap();
    assert_eq!(symtab.info as usize, first_global);
}

#[test]
fn relocations_are_pie_friendly() {
    let bytes = object(greeter());
    let relocs = Elf::parse(&bytes).relocations();
    let kinds: Vec<(u32, &str, i64)> = relocs
        .iter()
        .map(|r| (r.kind, r.symbol.as_str(), r.addend))
        .collect();
    // The string is reached RIP-relatively; `puts` through the PLT. The
    // call to the internal helper is resolved in place (no relocation).
    assert_eq!(
        kinds,
        [(R_X86_64_PC32, ".rodata", -4), (R_X86_64_PLT32, "puts", -4)]
    );
}

#[test]
fn output_is_deterministic() {
    assert_eq!(object(greeter()), object(greeter()));
}

#[test]
fn empty_module_is_a_valid_object() {
    let bytes = object(Module::new());
    let elf = Elf::parse(&bytes);
    assert_eq!(elf.section(".text"), Some(&[][..]));
    assert!(elf.find(".rodata").is_none());
    assert!(elf.find(".rela.text").is_none());
    assert_eq!(elf.symbols().len(), 2, "null + .text section symbol");
}

#[test]
fn readelf_accepts_the_object() {
    if Command::new("readelf").arg("--version").output().is_err() {
        eprintln!("skipping: `readelf` not available");
        return;
    }
    let dir = temp_dir();
    let path = dir.join("greeter.o");
    std::fs::write(&path, object(greeter())).unwrap();
    let out = Command::new("readelf")
        .args(["-a", "-W"])
        .arg(&path)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success());
    assert!(
        out.stderr.is_empty(),
        "readelf complained:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn symbol_names_with_nul_are_rejected() {
    let mut module = Module::new();
    let sym = module.symbols_mut().intern("bad\0name");
    let mut f = Function::new(sym, Linkage::Exported, Abi::C, vec![], TypeId::VOID, false);
    let mut block = astronomy::BasicBlock::new();
    block.terminator = Some(astronomy::Terminator::Return { value: None });
    f.blocks.push(block);
    module.functions_mut().push(f);
    let verified = Verifier::verify(module).unwrap();
    let err = astronomy_object::compile_object(&verified).unwrap_err();
    assert_eq!(err.code(), "A-OBJ-005", "{err}");
}

/// A loop whose body is far larger than 127 bytes forces both the backward
/// `jmp` and the loop-exit branch into their 32-bit forms.
#[test]
fn long_branches_are_relaxed() {
    let mut b = ModuleBuilder::with_name("loop");
    let f = b
        .declare_function("sum_to", Linkage::Exported, Abi::C, &[("n", I64)], I64)
        .unwrap();
    {
        let mut fb = b.function_builder(f).unwrap();
        let n = fb.param(0);
        let entry = fb.append_block();
        let head = fb
            .append_block_with_params(&[("acc", I64), ("i", I64)])
            .unwrap();
        let body = fb
            .append_block_with_params(&[("acc", I64), ("i", I64)])
            .unwrap();
        let exit = fb.append_block_with_params(&[("r", I64)]).unwrap();

        fb.switch_to(entry).unwrap();
        let zero = fb.const_int(I64, 0).unwrap();
        fb.jump(head, &[zero, zero]).unwrap();

        let (acc, i) = (
            fb.block_param(head, 0).unwrap(),
            fb.block_param(head, 1).unwrap(),
        );
        fb.switch_to(head).unwrap();
        let more = fb.lt(i, n).unwrap();
        fb.branch(more, body, &[acc, i], exit, &[acc]).unwrap();

        let (acc, i) = (
            fb.block_param(body, 0).unwrap(),
            fb.block_param(body, 1).unwrap(),
        );
        fb.switch_to(body).unwrap();
        let one = fb.const_int(I64, 1).unwrap();
        let mut sum = fb.add(acc, i).unwrap();
        for _ in 0..24 {
            sum = fb.mul(sum, one).unwrap();
        }
        let next = fb.add(i, one).unwrap();
        fb.jump(head, &[sum, next]).unwrap();

        let r = fb.block_param(exit, 0).unwrap();
        fb.switch_to(exit).unwrap();
        fb.ret(Some(r)).unwrap();
    }
    let module = b.finish();

    // The loop body alone must exceed the rel8 range for the test to mean
    // anything; NASM cross-checks the chosen encodings in `assert_prints`.
    let bytes = object(module.clone());
    let text_len = Elf::parse(&bytes).section(".text").unwrap().len();
    assert!(text_len > 300, "loop body too small: {text_len} bytes");

    assert_prints(
        module,
        "#include <stdio.h>\nextern long long sum_to(long long);\n\
         int main(void){ printf(\"%lld\\n\", sum_to(100)); return 0; }",
        "4950",
    );
}

/// The object needs no text relocations, so it can go into a shared
/// library (`-z text` makes the linker reject any), and the library works.
#[test]
fn links_into_a_shared_library() {
    if !gcc_available() {
        eprintln!("skipping: `gcc` not available");
        return;
    }
    let dir = temp_dir();
    std::fs::write(dir.join("greeter.o"), object(greeter())).unwrap();
    let lib = Command::new("gcc")
        .args(["-shared", "-Wl,-z,text", "-o"])
        .arg(dir.join("libgreeter.so"))
        .arg(dir.join("greeter.o"))
        .output()
        .unwrap();
    assert!(
        lib.status.success() && lib.stderr.is_empty(),
        "shared link failed:\n{}",
        String::from_utf8_lossy(&lib.stderr)
    );

    std::fs::write(
        dir.join("main.c"),
        "#include <stdio.h>\nextern long long greet(long long);\n\
         int main(void){ printf(\"%lld\\n\", greet(21)); return 0; }\n",
    )
    .unwrap();
    let exe = Command::new("gcc")
        .arg(dir.join("main.c"))
        .arg("-L")
        .arg(&dir)
        .arg(format!("-Wl,-rpath,{}", dir.display()))
        .args(["-lgreeter", "-o"])
        .arg(dir.join("main"))
        .output()
        .unwrap();
    assert!(
        exe.status.success(),
        "{}",
        String::from_utf8_lossy(&exe.stderr)
    );
    let run = Command::new(dir.join("main")).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "hello from a built-in object\n42\n"
    );
}
