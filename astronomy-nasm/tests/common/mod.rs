//! Shared helpers for the `astronomy-nasm` integration tests.
//!
//! Tests build Astronomy IR in-memory, lower it with the backend, assemble
//! it with NASM, link it against a tiny C driver and run it. When `nasm` or
//! `gcc` is unavailable the execution helpers return `None` and the test
//! prints a skip message instead of failing.

#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use astronomy::{
    Abi, FunctionBuilder, Linkage, Module, ModuleBuilder, TypeId, Verifier,
};

/// Verifies and lowers a module, returning the NASM text.
pub fn compile_ok(module: Module) -> String {
    let verified = Verifier::verify(module).expect("module must verify");
    astronomy_nasm::compile(&verified).expect("module must lower to NASM")
}

/// Verifies then lowers, expecting a backend error.
pub fn compile_err(module: Module) -> astronomy_nasm::BackendError {
    let verified = Verifier::verify(module).expect("module must verify");
    astronomy_nasm::compile(&verified).expect_err("expected a backend error")
}

/// Builds a module with a single exported `abi=c` function.
pub fn one_function<F>(
    name: &str,
    params: &[(&str, TypeId)],
    result: TypeId,
    body: F,
) -> Module
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

/// True when the external toolchain needed for execution tests is present.
pub fn tools_available() -> bool {
    let nasm = Command::new("nasm").arg("-v").output();
    let gcc = Command::new("gcc").arg("--version").output();
    matches!(nasm, Ok(o) if o.status.success()) && matches!(gcc, Ok(o) if o.status.success())
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "astronomy-nasm-{}-{n}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Assembles and links `module` with `driver` C source, then runs it.
///
/// Returns `None` when the toolchain is unavailable. Panics with the full
/// assembly listing when NASM or GCC fails, so a codegen bug is immediately
/// visible in the test output.
pub fn assemble_and_run(module: Module, driver: &str) -> Option<RunOutput> {
    if !tools_available() {
        eprintln!("skipping execution test: `nasm`/`gcc` not available");
        return None;
    }

    let verified = Verifier::verify(module).expect("module must verify");
    let asm = astronomy_nasm::compile(&verified).expect("module must lower to NASM");

    let dir = temp_dir();
    let asm_path = dir.join("module.asm");
    std::fs::write(&asm_path, &asm).unwrap();
    let obj_path = dir.join("module.o");

    let nasm = Command::new("nasm")
        .args(["-f", "elf64"])
        .arg(&asm_path)
        .arg("-o")
        .arg(&obj_path)
        .output()
        .expect("failed to spawn nasm");
    assert!(
        nasm.status.success(),
        "nasm failed:\n{}\n--- assembly ---\n{asm}",
        String::from_utf8_lossy(&nasm.stderr)
    );

    let c_path = dir.join("driver.c");
    std::fs::write(&c_path, driver).unwrap();
    let prog_path = dir.join("prog");
    let gcc = Command::new("gcc")
        .args(["-no-pie", "-O0"])
        .arg(&c_path)
        .arg(&obj_path)
        .arg("-o")
        .arg(&prog_path)
        .output()
        .expect("failed to spawn gcc");
    assert!(
        gcc.status.success(),
        "gcc failed:\n{}\n--- assembly ---\n{asm}",
        String::from_utf8_lossy(&gcc.stderr)
    );

    let run = Command::new(&prog_path).output().expect("failed to run program");
    let result = RunOutput {
        code: run.status.code(),
        stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&run.stderr).into_owned(),
    };
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        run.status.success(),
        "compiled program failed: {}\nstdout: {}\nstderr: {}\n--- assembly ---\n{asm}",
        run.status, result.stdout, result.stderr
    );
    Some(result)
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
