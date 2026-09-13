//! `arn2nasm` — transcompile a canonical `.arn` module to NASM assembly.
//!
//! ```text
//! arn2nasm hello.arn > hello.asm
//! arn2nasm < hello.arn
//! ```
//!
//! The pipeline is exactly the library pipeline: parse `.arn` → verify →
//! lower to NASM. Anything invalid fails with a structured, coded error.

use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = match args.as_slice() {
        [] => {
            let mut buf = String::new();
            if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("error: cannot read stdin: {e}");
                return ExitCode::FAILURE;
            }
            buf
        }
        [flag] if flag == "-h" || flag == "--help" => {
            println!("usage: arn2nasm [file.arn]   (reads stdin when omitted)");
            return ExitCode::SUCCESS;
        }
        [path] => match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read `{path}`: {e}");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!("usage: arn2nasm [file.arn]");
            return ExitCode::FAILURE;
        }
    };

    let module = match astronomy::text::parse(&source) {
        Ok(module) => module,
        Err(e) => {
            eprintln!("error[{}]: {e}", e.code());
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
