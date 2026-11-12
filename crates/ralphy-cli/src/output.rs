//! The one module of the CLI that prints to stdout and stderr. Every other
//! module prints through the macros below, which keep the stream, the text and
//! the newline of `print!`, `println!` and `eprintln!`.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the CLI talks to its user and to scripts on stdout and stderr; \
              this module is the one place that writes to them"
)]

use std::fmt::Arguments;

/// `print!`: the text on stdout, with no newline.
pub(crate) fn text(args: Arguments<'_>) {
    print!("{args}");
}

/// `println!`: the text and a newline on stdout.
pub(crate) fn line(args: Arguments<'_>) {
    println!("{args}");
}

/// `eprintln!`: the text and a newline on stderr.
pub(crate) fn err_line(args: Arguments<'_>) {
    eprintln!("{args}");
}

/// `print!` through [`text()`].
macro_rules! out {
    ($($arg:tt)*) => {
        $crate::output::text(format_args!($($arg)*))
    };
}

/// `println!` through [`line()`].
macro_rules! outln {
    () => {
        $crate::output::line(format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::output::line(format_args!($($arg)*))
    };
}

/// `eprintln!` through [`err_line()`].
macro_rules! eoutln {
    () => {
        $crate::output::err_line(format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::output::err_line(format_args!($($arg)*))
    };
}
