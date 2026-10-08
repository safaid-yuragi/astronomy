//! `arn2obj` — compile an Astronomy module to an x86-64 Linux object file.
//!
//! ```text
//! arn2obj hello.arn                  # writes hello.o (like `cc -c`)
//! arn2obj hello.arb -o out.o         # .arb input, explicit output
//! arn2obj -o hello.o < hello.arn     # stdin input
//! ```
//!
//! The input may be `.arn` text or `.arb` binary (detected by content). The
//! pipeline is exactly the library pipeline: load → verify → lower → encode
//! an ELF object with the built-in encoder. Anything invalid fails with a
//! structured, coded error.

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

const USAGE: &str = "\
usage: arn2obj [-o FILE] [FILE]

  FILE      .arn text or .arb binary (stdin when omitted)
  -o FILE   output object file (`-` for stdout); by default FILE's name
            with a `.o` extension in the current directory, or stdout
            when reading stdin
";

struct Options {
    input: Option<String>,
    output: Option<String>,
}

fn parse_args() -> Result<Options, String> {
    let mut opts = Options {
        input: None,
        output: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" => opts.output = Some(args.next().ok_or("`-o` needs a file name")?),
            "-h" | "--help" => return Err(String::new()),
            flag if flag.starts_with('-') && flag != "-" => {
                return Err(format!("unknown option `{flag}`"))
            }
            path if opts.input.is_none() => opts.input = Some(path.to_string()),
            _ => return Err("only one input file may be given".to_string()),
        }
    }
    Ok(opts)
}

fn run(opts: Options) -> Result<(), String> {
    let source = match opts.input.as_deref() {
        None | Some("-") => {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| format!("error: cannot read stdin: {e}"))?;
            buf
        }
        Some(path) => {
            std::fs::read(path).map_err(|e| format!("error: cannot read `{path}`: {e}"))?
        }
    };

    let loaded = if astronomy::binary::is_arb(&source) {
        astronomy::binary::read(&source).map_err(|e| (e.code(), e.to_string()))
    } else {
        let text = String::from_utf8(source)
            .map_err(|_| "error: input is neither .arb binary nor UTF-8 .arn text".to_string())?;
        astronomy::text::parse(&text).map_err(|e| (e.code(), e.to_string()))
    };
    let module = loaded.map_err(|(code, msg)| format!("error[{code}]: {msg}"))?;
    let verified = module.verify().map_err(|report| report.to_string())?;
    let object =
        astronomy_object::compile(&verified).map_err(|e| format!("error[{}]: {e}", e.code()))?;

    let output = opts.output.or_else(|| match opts.input.as_deref() {
        Some(input) if input != "-" => Some(default_object_name(input)),
        _ => None,
    });
    match output.as_deref() {
        Some(path) if path != "-" => {
            std::fs::write(path, &object).map_err(|e| format!("error: cannot write `{path}`: {e}"))
        }
        _ => {
            let mut stdout = std::io::stdout();
            if stdout.is_terminal() {
                return Err(
                    "error: refusing to write an object file to a terminal; use `-o FILE`"
                        .to_string(),
                );
            }
            stdout
                .write_all(&object)
                .and_then(|()| stdout.flush())
                .map_err(|e| format!("error: cannot write output: {e}"))
        }
    }
}

/// `dir/hello.arn` → `hello.o` (in the current directory, like `cc -c`).
fn default_object_name(input: &str) -> String {
    let stem = std::path::Path::new(input)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "out".to_string());
    format!("{stem}.o")
}

fn main() -> ExitCode {
    let opts = match parse_args() {
        Ok(opts) => opts,
        Err(msg) if msg.is_empty() => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("error: {msg}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}
