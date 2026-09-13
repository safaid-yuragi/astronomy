//! End-to-end CLI test: `arn2nasm` parses `.arn`, verifies and emits NASM,
//! exercising the same pipeline a user would drive from the shell.

use std::process::Command;

const ADD_ARN: &str = "\
::ASTRONOMY::MODULE_START
::ASTRONOMY::MODULE_NAME \"add\"
::ASTRONOMY::MODULE_VERSION 1
::ASTRONOMY::FUNCTION_START add fn(i64 %a, i64 %b) -> i64 linkage=exported abi=c
::ASTRONOMY::BLOCK_START bb0
%0 = ::ASTRONOMY::ADD i64 i64 %a, i64 %b
::ASTRONOMY::RETURN i64 %0
::ASTRONOMY::BLOCK_END
::ASTRONOMY::FUNCTION_END
::ASTRONOMY::MODULE_END
";

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("astronomy-nasm-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn cli_transcompiles_a_file() {
    let path = scratch("add.arn");
    std::fs::write(&path, ADD_ARN).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_arn2nasm"))
        .arg(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let asm = String::from_utf8(out.stdout).unwrap();
    assert!(asm.contains("global $add"), "{asm}");
    assert!(asm.contains("    add rax, rcx"), "{asm}");
    assert!(asm.contains("    ret"), "{asm}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cli_reads_stdin() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_arn2nasm"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(ADD_ARN.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("add:"));
}

#[test]
fn cli_rejects_invalid_arn() {
    let path = scratch("bad.arn");
    std::fs::write(&path, "::ASTRONOMY::MODULE_START\n::ASTRONOMY::BOGUS\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_arn2nasm"))
        .arg(&path)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("A-ARN"));
    let _ = std::fs::remove_file(&path);
}
