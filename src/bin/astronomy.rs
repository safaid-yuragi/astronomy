//! Minimal development CLI (§41 CLI is Optional).
//!
//! A small helper for debugging `.arn` files. The library API is never
//! shaped around this binary.

use std::process::ExitCode;

const USAGE: &str = "\
astronomy — Astronomy IR development tools

USAGE:
  astronomy verify   <file>            Load and verify a module
  astronomy fmt      <file>            Load and print canonical ARN
  astronomy inspect  <file>            Summarize a module
  astronomy encode   <file> <out.arb>  Write the module as .arb binary
  astronomy help                       Show this help

<file> may be .arn text or .arb binary (detected by content).
";

enum Command {
    Verify(String),
    Fmt(String),
    Inspect(String),
    Encode(String, String),
}

fn parse_args() -> Result<Command, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, path] if cmd == "verify" => Ok(Command::Verify(path.clone())),
        [cmd, path] if cmd == "fmt" => Ok(Command::Fmt(path.clone())),
        [cmd, path] if cmd == "inspect" => Ok(Command::Inspect(path.clone())),
        [cmd, input, output] if cmd == "encode" => {
            Ok(Command::Encode(input.clone(), output.clone()))
        }
        [cmd] if cmd == "help" || cmd == "--help" || cmd == "-h" => Err(USAGE.to_string()),
        _ => Err(USAGE.to_string()),
    }
}

/// Loads `.arb` binary or `.arn` text, detected by the `.arb` magic bytes.
fn load(path: &str) -> Result<astronomy::Module, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read `{path}`: {e}"))?;
    if astronomy::binary::is_arb(&bytes) {
        return astronomy::binary::read(&bytes).map_err(|e| e.to_string());
    }
    let source = String::from_utf8(bytes).map_err(|_| format!("`{path}` is not UTF-8 text"))?;
    astronomy::text::parse(&source).map_err(|e| format!("{}: {e}", e.code()))
}

fn run(cmd: Command) -> Result<(), String> {
    match cmd {
        Command::Verify(path) => {
            let module = load(&path)?;
            match module.verify() {
                Ok(verified) => {
                    println!("OK: {} function(s) verified", verified.functions().len());
                    Ok(())
                }
                Err(report) => {
                    for e in report.errors() {
                        eprintln!("error[{}]: {e}", e.code());
                    }
                    Err(format!("verification failed with {} error(s)", report.len()))
                }
            }
        }
        Command::Fmt(path) => {
            let module = load(&path)?;
            print!("{}", astronomy::text::print(&module));
            Ok(())
        }
        Command::Encode(input, output) => {
            let module = load(&input)?;
            let bytes = module.to_arb();
            std::fs::write(&output, &bytes)
                .map_err(|e| format!("cannot write `{output}`: {e}"))?;
            println!("wrote {} byte(s) to {output}", bytes.len());
            Ok(())
        }
        Command::Inspect(path) => {
            let module = load(&path)?;
            println!(
                "module: {}",
                module.name().unwrap_or("<unnamed>")
            );
            println!("version: {}", module.version());
            println!("types: {}", module.types().len());
            println!("constants: {}", module.constants().len());
            println!("functions: {}", module.functions().len());
            for f in module.functions() {
                let name = module.symbol_name(f.symbol).unwrap_or("<invalid>");
                let kind = if f.is_declaration() { "extern" } else { "def" };
                let blocks = f.blocks.len();
                let insts: usize = f.blocks.iter().map(|b| b.instructions.len()).sum();
                let values = f.values.len();
                println!(
                    "  {kind} {name}: blocks={blocks} insts={insts} values={values}"
                );
            }
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) if msg == USAGE => {
            print!("{USAGE}");
            ExitCode::from(2)
        }
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}
