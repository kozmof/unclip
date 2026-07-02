//! Fallible CLI output helpers.

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

fn write_to(mut writer: impl Write, contents: &str) -> io::Result<()> {
    writer.write_all(contents.as_bytes())?;
    writer.flush()
}

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
    }
}
