//! Test helper child for `ralphy-pty`'s integration tests: prints each argument
//! it received as `ARG=<value>|` on its own line, then `ARGS-END`, and exits 0.
//! A test starts it directly or behind a `.cmd` shim to see what reached it.

use std::io::Write;

fn main() -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    for arg in std::env::args_os().skip(1) {
        writeln!(out, "ARG={}|", arg.to_string_lossy())?;
    }
    writeln!(out, "ARGS-END")?;
    out.flush()
}
