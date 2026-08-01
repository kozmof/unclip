//! Command handlers for the unclip CLI, grouped by what they act on.
//!
//! - [`branch`] — create, change, and delete branches (`add`, `edit`, `rm`,
//!   `import`, `attach`, `refs`).
//! - [`navigate`] — read the archive without changing it (`show`, `ls`,
//!   `tree`, `query`, `o2o`, `o2m`).
//! - [`frames`] — frame definitions and the commands built on them
//!   (`import-frames`, `frames`, `frame`, `rm-frame`, `create`, `validate`).
//!
//! Sampling (`sample`, `compose`, `replay`, `export`) lives in
//! [`crate::sampling`], and usage reporting in [`crate::usage`].

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;

use anyhow::{bail, Context};
use unclip_core::Branch;

pub mod branch;
pub mod frames;
pub mod navigate;

pub use branch::{add, attach, edit, import, refs, rm, AddInput, EditInput};
pub use frames::{create, frame_show, frames_list, import_frames, rm_frame, validate};
pub use navigate::{ls, o2m, o2o, query, show, tree, QueryInput};

/// Parse a `name=value` pair, used for `--o2o` / `--o2m` flags.
pub fn parse_kv(raw: &str) -> anyhow::Result<(String, String)> {
    let (name, value) = raw
        .split_once('=')
        .with_context(|| format!("expected name=value, got `{raw}`"))?;
    if name.is_empty() {
        bail!("empty name in `{raw}`");
    }
    if value.is_empty() {
        bail!("empty value in `{raw}`");
    }
    Ok((name.to_string(), value.to_string()))
}

/// Merge `name=value` pairs into a one-to-one o2o map, rejecting any name that
/// is already present. o2o values are one-to-one, so a repeated name
/// — whether across flags or colliding with a frame slot's base value — is a
/// usage error.
pub fn merge_o2o(
    map: &mut BTreeMap<String, String>,
    pairs: Vec<(String, String)>,
) -> anyhow::Result<()> {
    for (name, value) in pairs {
        match map.entry(name) {
            Entry::Occupied(entry) => {
                bail!(
                    "duplicate o2o name `{}` (o2o values are one-to-one)",
                    entry.key()
                );
            }
            Entry::Vacant(entry) => {
                entry.insert(value);
            }
        }
    }
    Ok(())
}

/// One `path[\ttitle]` line, the shared listing format of `query`, `o2o`, and
/// `o2m`.
pub(crate) fn print_path_line(branch: &Branch) -> anyhow::Result<()> {
    match &branch.title {
        Some(title) => crate::output::outln!("{}\t{}", branch.path, title),
        None => crate::output::outln!("{}", branch.path),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kv_splits_on_first_equals() {
        assert_eq!(
            parse_kv("place=cafe").unwrap(),
            ("place".to_string(), "cafe".to_string())
        );
        // Only the first `=` is a separator; later ones belong to the value.
        assert_eq!(
            parse_kv("expr=a=b").unwrap(),
            ("expr".to_string(), "a=b".to_string())
        );
    }

    #[test]
    fn parse_kv_rejects_missing_separator_or_empty_parts() {
        assert!(parse_kv("nope").is_err());
        assert!(parse_kv("=value").is_err());
        assert!(parse_kv("name=").is_err());
    }

    #[test]
    fn merge_o2o_inserts_distinct_names() {
        let mut map = BTreeMap::new();
        merge_o2o(
            &mut map,
            vec![
                ("place".into(), "cafe".into()),
                ("mood".into(), "calm".into()),
            ],
        )
        .unwrap();
        assert_eq!(map.get("place").map(String::as_str), Some("cafe"));
        assert_eq!(map.get("mood").map(String::as_str), Some("calm"));
    }

    #[test]
    fn merge_o2o_rejects_duplicate_name() {
        let mut map = BTreeMap::new();
        map.insert("place".to_string(), "cafe".to_string());
        // Colliding with an existing entry (e.g. a frame slot's base value) is a
        // usage error because o2o is one-to-one.
        let err = merge_o2o(&mut map, vec![("place".into(), "park".into())]).unwrap_err();
        assert!(err.to_string().contains("duplicate o2o name `place`"));
        // The colliding pair is rejected without touching the existing entry.
        assert_eq!(map.get("place").map(String::as_str), Some("cafe"));
    }

    #[test]
    fn merge_o2o_rejects_duplicate_within_same_batch() {
        let mut map = BTreeMap::new();
        let err = merge_o2o(
            &mut map,
            vec![("k".into(), "1".into()), ("k".into(), "2".into())],
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate o2o name `k`"));
    }
}
