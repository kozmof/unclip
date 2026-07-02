//! Bounded UTF-8 file loading for user-supplied CLI inputs.

use std::io::Read;
use std::path::Path;

use anyhow::Context;

/// Maximum size accepted for a text input parsed or scanned in memory.
pub const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;

/// Read a UTF-8 text file without allowing an unbounded allocation.
pub fn read_text_file(path: &Path, description: &str) -> anyhow::Result<String> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open {description} {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {description} {}", path.display()))?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_TEXT_BYTES,
        "{description} {} exceeds the {} MiB limit",
        path.display(),
        MAX_TEXT_BYTES / 1024 / 1024
    );
    String::from_utf8(bytes)
        .with_context(|| format!("{description} {} is not valid UTF-8", path.display()))
}
