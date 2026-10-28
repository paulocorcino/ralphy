//! Test helper child that stands in for `claude --version`.
//!
//! A version read runs the program with `--version` only, so the mode comes
//! from the arguments in the file `<this exe>.args` (one per line) when there
//! is one, else from argv. Modes: `print <text>` prints the text;
//! `count <file> <text>` appends one line to the file, then prints the text;
//! `hang` sleeps 60 s; `exit0` exits 0 and writes nothing. Every mode except
//! `hang` ignores a trailing `--version`.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = mode_args();
    match args.first().map(String::as_str).unwrap_or("") {
        "print" => {
            println!("{}", args.get(1).map(String::as_str).unwrap_or(""));
            ExitCode::SUCCESS
        }
        "count" => count(&args),
        "hang" => {
            std::thread::sleep(std::time::Duration::from_secs(60));
            ExitCode::SUCCESS
        }
        "exit0" => ExitCode::SUCCESS,
        _ => ExitCode::from(2),
    }
}

fn mode_args() -> Vec<String> {
    let sidecar = std::env::current_exe().ok().map(|exe| {
        let mut name = exe.into_os_string();
        name.push(".args");
        std::path::PathBuf::from(name)
    });
    match sidecar.and_then(|f| std::fs::read_to_string(f).ok()) {
        Some(text) => text.lines().map(str::to_string).collect(),
        None => std::env::args().skip(1).collect(),
    }
}

fn count(args: &[String]) -> ExitCode {
    let (Some(file), Some(text)) = (args.get(1), args.get(2)) else {
        return ExitCode::from(2);
    };
    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut f| writeln!(f, "run"));
    if appended.is_err() {
        return ExitCode::from(3);
    }
    println!("{text}");
    ExitCode::SUCCESS
}
