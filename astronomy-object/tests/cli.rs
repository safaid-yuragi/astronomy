//! End-to-end CLI test: `arn2obj` loads `.arn`/`.arb`, verifies and writes
//! an ELF object, exercising the same pipeline a user would drive from the
//! shell.

use std::process::{Command, Stdio};

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
    let dir = std::env::temp_dir().join(format!("astronomy-object-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn arn2obj() -> Command {
    Command::new(env!("CARGO_BIN_EXE_arn2obj"))
}

/// Asserts `bytes` is an x86-64 ELF relocatable object.
fn assert_object(bytes: &[u8]) {
    assert_eq!(&bytes[..4], b"\x7fELF", "not an ELF file");
    assert_eq!(u16::from_le_bytes([bytes[16], bytes[17]]), 1, "ET_REL");
    assert_eq!(u16::from_le_bytes([bytes[18], bytes[19]]), 62, "EM_X86_64");
}

#[test]
fn cli_writes_the_requested_file() {
    let input = scratch("obj.arn");
    let output = scratch("obj.o");
    std::fs::write(&input, ADD_ARN).unwrap();
    let out = arn2obj()
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_object(&std::fs::read(&output).unwrap());
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
}

#[test]
fn cli_names_the_object_after_its_input() {
    // Like `cc -c`: `dir/named.arn` becomes `named.o` in the current directory.
    let input = scratch("named.arn");
    std::fs::write(&input, ADD_ARN).unwrap();
    let cwd = scratch("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let out = arn2obj().arg(&input).current_dir(&cwd).output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    assert_object(&std::fs::read(cwd.join("named.o")).expect("named.o is written"));
    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_file(&input);
}

#[test]
fn cli_reads_stdin_and_writes_stdout() {
    use std::io::Write;
    let mut child = arn2obj()
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
    assert_object(&out.stdout);
}

#[test]
fn cli_accepts_arb_binary() {
    let module = astronomy::Module::parse_arn(ADD_ARN).unwrap();
    let input = scratch("add.arb");
    let output = scratch("add-arb.o");
    std::fs::write(&input, module.to_arb()).unwrap();
    let out = arn2obj()
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_object(&std::fs::read(&output).unwrap());
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
}

#[test]
fn cli_rejects_invalid_arn() {
    let path = scratch("bad.arn");
    std::fs::write(&path, "::ASTRONOMY::MODULE_START\n::ASTRONOMY::BOGUS\n").unwrap();
    let out = arn2obj()
        .arg(&path)
        .arg("-o")
        .arg(scratch("bad.o"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("A-ARN"));
    assert!(!scratch("bad.o").exists(), "no output on failure");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cli_rejects_corrupt_arb() {
    let module = astronomy::Module::parse_arn(ADD_ARN).unwrap();
    let mut bytes = module.to_arb();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    let path = scratch("corrupt.arb");
    std::fs::write(&path, bytes).unwrap();
    let out = arn2obj().arg(&path).arg("-o").arg("-").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("A-ARB-005"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn cli_rejects_unknown_options() {
    let out = arn2obj().arg("--emit=asm").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option"));
}
