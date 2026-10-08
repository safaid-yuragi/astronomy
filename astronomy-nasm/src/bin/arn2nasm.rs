//! `arn2nasm` — transcompile an Astronomy module to NASM assembly.
//!
//! ```text
//! arn2nasm hello.arn > hello.asm
//! arn2nasm hello.arb > hello.asm
//! arn2nasm < hello.arn
//! ```
//!
//! The input may be `.arn` text or `.arb` binary (detected by content). The
//! pipeline is exactly the library pipeline: load → verify → lower to NASM.
//! Anything invalid fails with a structured, coded error.

use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = match args.as_slice() {
        [] => {
            let mut buf = Vec::new();
            if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
                eprintln!("error: cannot read stdin: {e}");
                return ExitCode::FAILURE;
            }
            buf
        }
        [flag] if flag == "-h" || flag == "--help" => {
            println!("usage: arn2nasm [file.arn|file.arb]   (reads stdin when omitted)");
            return ExitCode::SUCCESS;
        }
        [path] => match std::fs::read(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read `{path}`: {e}");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: arn2nasm [file.arn|file.arb]");
            return ExitCode::FAILURE;
        }
    };

    let loaded = if astronomy::binary::is_arb(&source) {
        astronomy::binary::read(&source).map_err(|e| (e.code(), e.to_string()))
    } else {
        match String::from_utf8(source) {
            Ok(text) => astronomy::text::parse(&text).map_err(|e| (e.code(), e.to_string())),
            Err(_) => {
                eprintln!("error: input is neither .arb binary nor UTF-8 .arn text");
                return ExitCode::FAILURE;
            }
        }
    };
    let module = match loaded {
        Ok(module) => module,
        Err((code, msg)) => {
            eprintln!("error[{code}]: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let verified = match module.verify() {
        Ok(verified) => verified,
        Err(report) => {
            eprintln!("{report}");
            return ExitCode::FAILURE;
        }
    };
    match astronomy_nasm::compile(&verified) {
        Ok(asm) => {
            print!("{asm}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error[{}]: {e}", e.code());
            ExitCode::FAILURE
        }
    }
}
