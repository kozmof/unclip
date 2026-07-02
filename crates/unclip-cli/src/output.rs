//! Fallible CLI output helpers.

use std::fmt;
use std::io::{self, Write};

use anyhow::Context;

/// Write a complete serialized value to stdout and surface pipe/write failures.
///
/// `print!` panics when stdout fails. Returning the error lets command handlers
/// avoid recording side effects when a downstream consumer did not accept the
/// packet.
pub fn write_stdout(contents: &str) -> anyhow::Result<()> {
    write_to(io::stdout().lock(), contents).context("failed to write stdout")
}

/// Write formatted output to stdout without using the panic-on-error standard
/// printing macros.
pub fn write_stdout_fmt(args: fmt::Arguments<'_>) -> anyhow::Result<()> {
    write_fmt_to(io::stdout().lock(), args).context("failed to write stdout")
}

pub fn write_stdout_line(args: fmt::Arguments<'_>) -> anyhow::Result<()> {
    write_line_to(io::stdout().lock(), args).context("failed to write stdout")
}

pub fn write_stderr_line(args: fmt::Arguments<'_>) -> anyhow::Result<()> {
    write_line_to(io::stderr().lock(), args).context("failed to write stderr")
}

fn write_to(mut writer: impl Write, contents: &str) -> io::Result<()> {
    writer.write_all(contents.as_bytes())?;
    writer.flush()
}

fn write_fmt_to(mut writer: impl Write, args: fmt::Arguments<'_>) -> io::Result<()> {
    writer.write_fmt(args)?;
    writer.flush()
}

fn write_line_to(mut writer: impl Write, args: fmt::Arguments<'_>) -> io::Result<()> {
    writer.write_fmt(args)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

macro_rules! out {
    ($($arg:tt)*) => {{
        $crate::output::write_stdout_fmt(format_args!($($arg)*))?;
    }};
}

macro_rules! outln {
    () => {{
        $crate::output::write_stdout_line(format_args!(""))?;
    }};
    ($($arg:tt)*) => {{
        $crate::output::write_stdout_line(format_args!($($arg)*))?;
    }};
}

macro_rules! errln {
    () => {{
        $crate::output::write_stderr_line(format_args!(""))?;
    }};
    ($($arg:tt)*) => {{
        $crate::output::write_stderr_line(format_args!($($arg)*))?;
    }};
}

pub(crate) use {errln, out, outln};

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenWriter;

    impl Write for BrokenWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_to_propagates_output_failures() {
        let error = write_to(BrokenWriter, "packet").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

        let error = write_line_to(BrokenWriter, format_args!("{}", "line")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
