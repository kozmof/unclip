//! Validation of branches and packets against frame constraints.
//!
//! Validation reports a list of human-readable violation reasons. An empty
//! list means the subject satisfies the constraints. Only *hard* constraints
//! are checked (scope / require_o2o / avoid_o2o / require_o2m / avoid_o2m);
//! `prefer_o2m` is a scoring signal, not a requirement.

use crate::branch::is_under;
use crate::error::{CoreError, Result};
use crate::frame::{Frame, Slot};
use crate::packet::SelectionPacket;
use crate::{Branch, Reference, PACKET_KIND, PACKET_VERSION};

/// Maximum UTF-8 size of a branch path.
pub const MAX_PATH_BYTES: usize = 4 * 1024;
/// Maximum UTF-8 size of one string stored inside a domain record.
pub const MAX_DOMAIN_STRING_BYTES: usize = 64 * 1024;
/// Maximum aggregate size of one branch record.
pub const MAX_BRANCH_RECORD_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of indexed values and references carried by one branch.
pub const MAX_BRANCH_COLLECTION_ITEMS: usize = 10_000;
/// Maximum number of names and values carried by one frame.
pub const MAX_FRAME_COLLECTION_ITEMS: usize = 10_000;
/// Maximum aggregate complexity accepted in one query.
///
/// This stays below SQLite's historical 999-variable limit even when a filter
/// shape binds each logical item more than once.
pub const MAX_QUERY_FILTER_ITEMS: usize = 400;

/// Why a domain string is not storable.
///
/// Returned rather than rendered, so each caller words the failure in its own
/// vocabulary — "o2o name", "reference type", "target value" — while the rule
/// itself is stated once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainStringFault {
    Empty,
    Oversized,
    ControlCharacters,
}

/// The rule every stored name and value satisfies: non-empty, within
/// [`MAX_DOMAIN_STRING_BYTES`], and free of control characters.
///
/// This was the same three-condition `if` written out seven times — for o2o
/// names and values, o2m names and values, reference types and values, and
/// pattern targets — each with its own hand-written message. Seven copies of a
/// rule is seven places to forget a condition when the rule changes, and the
/// copies had already drifted: the branch title/description checks test the
/// same two bounds in the opposite order and report them separately.
pub fn validate_domain_string(value: &str) -> std::result::Result<(), DomainStringFault> {
    if value.is_empty() {
        return Err(DomainStringFault::Empty);
    }
    if value.len() > MAX_DOMAIN_STRING_BYTES {
        return Err(DomainStringFault::Oversized);
    }
    if value.chars().any(char::is_control) {
        return Err(DomainStringFault::ControlCharacters);
    }
    Ok(())
}

/// Validate a branch path address.
///
/// A path must be absolute (`/`-prefixed), have no empty segments (no `//`),
/// no trailing slash, and contain no whitespace. The bare root `/` is not a
/// valid branch address.
///
/// `.` and `..` are rejected as segments. A branch path is a storage key, not a
/// filesystem path: nothing resolves it, so `/a/../b` would be a row distinct
/// from `/b` while reading as the same place, and `is_under` would report it as
/// scoped beneath `/a`. Refusing the segment is the only way the two readings
/// cannot disagree.
pub fn validate_path(path: &str) -> Result<()> {
    let invalid = |path: &str| CoreError::InvalidPath(path.to_string());

    if path.len() > MAX_PATH_BYTES || path == "/" || !path.starts_with('/') || path.ends_with('/') {
        return Err(invalid(path));
    }
    for segment in path.split('/').skip(1) {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control())
        {
            return Err(invalid(path));
        }
    }
    Ok(())
}

/// Validate branch invariants that must hold before persistence or sampling.
pub fn validate_branch_record(branch: &Branch) -> Result<()> {
    validate_path(&branch.path)?;

    let invalid = |reason: String| CoreError::InvalidBranch {
        path: branch.path.clone(),
        reason,
    };

    if !branch.weight.is_finite() {
        return Err(invalid(format!(
            "weight must be finite, got {}",
            branch.weight
        )));
    }
    if branch.weight < 0.0 {
        return Err(invalid(format!(
            "weight must be non-negative, got {}",
            branch.weight
        )));
    }

    for (name, field) in [
        ("title", branch.title.as_deref()),
        ("description", branch.description.as_deref()),
    ] {
        if field.is_some_and(|value| value.len() > MAX_DOMAIN_STRING_BYTES) {
            return Err(invalid(format!(
                "{name} exceeds the {MAX_DOMAIN_STRING_BYTES}-byte string limit"
            )));
        }
        if field.is_some_and(|value| value.chars().any(char::is_control)) {
            return Err(invalid(format!(
                "{name} must not contain control characters"
            )));
        }
    }

    let collection_items = branch
        .o2o
        .len()
        .saturating_add(branch.o2m.values().map(Vec::len).sum::<usize>())
        .saturating_add(branch.references.len());
    if collection_items > MAX_BRANCH_COLLECTION_ITEMS {
        return Err(invalid(format!(
            "branch contains more than {MAX_BRANCH_COLLECTION_ITEMS} indexed values and references"
        )));
    }

    for (name, value) in &branch.o2o {
        if validate_domain_string(name).is_err() {
            return Err(invalid(
                "o2o name must not be empty or contain control characters".to_string(),
            ));
        }
        if validate_domain_string(value).is_err() {
            return Err(invalid(format!(
                "o2o `{name}` value must not be empty or contain control characters"
            )));
        }
    }

    for (name, values) in &branch.o2m {
        if validate_domain_string(name).is_err() {
            return Err(invalid(
                "o2m name must not be empty or contain control characters".to_string(),
            ));
        }
        for value in values {
            if validate_domain_string(value).is_err() {
                return Err(invalid(format!(
                    "o2m `{name}` value must not be empty or contain control characters"
                )));
            }
        }
    }

    for reference in &branch.references {
        validate_reference(reference).map_err(|err| invalid(err.to_string()))?;
    }

    if branch_record_bytes(branch) > MAX_BRANCH_RECORD_BYTES {
        return Err(invalid(format!(
            "branch exceeds the {MAX_BRANCH_RECORD_BYTES}-byte record limit"
        )));
    }

    Ok(())
}

/// Validate a reference before storing it independently of a full branch.
pub fn validate_reference(reference: &Reference) -> Result<()> {
    if validate_domain_string(&reference.kind).is_err() {
        return Err(CoreError::InvalidBranch {
            path: "<reference>".to_string(),
            reason: "reference type must not be empty or contain control characters".to_string(),
        });
    }
    if validate_domain_string(&reference.value).is_err() {
        return Err(CoreError::InvalidBranch {
            path: "<reference>".to_string(),
            reason: "reference value must not be empty or contain control characters".to_string(),
        });
    }
    if reference
        .note
        .as_ref()
        .is_some_and(|note| note.len() > MAX_DOMAIN_STRING_BYTES)
    {
        return Err(CoreError::InvalidBranch {
            path: "<reference>".to_string(),
            reason: "reference note is oversized".to_string(),
        });
    }
    if reference
        .note
        .as_ref()
        .is_some_and(|note| note.chars().any(char::is_control))
    {
        return Err(CoreError::InvalidBranch {
            path: "<reference>".to_string(),
            reason: "reference note must not contain control characters".to_string(),
        });
    }
    Ok(())
}

/// Serialized byte length of a JSON value, without building the string.
///
/// `metadata.to_string()` would allocate the entire payload — up to the
/// [`MAX_BRANCH_RECORD_BYTES`] limit this function exists to enforce — only to
/// read its length and drop it. Counting into a sink measures the same bytes
/// `serde_json` would write while holding nothing.
fn json_bytes(value: &serde_json::Value) -> usize {
    /// Discards every byte and keeps the count.
    struct CountingSink(usize);

    impl std::io::Write for CountingSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut sink = CountingSink(0);
    // Serializing a `Value` to a sink that cannot fail has no failure mode of
    // its own, but a size that silently read as 0 would wave an oversized
    // record through, so fall back to the allocating path rather than guessing.
    if serde_json::to_writer(&mut sink, value).is_ok() {
        sink.0
    } else {
        value.to_string().len()
    }
}

pub(crate) fn branch_record_bytes(branch: &Branch) -> usize {
    let mut total = branch
        .path
        .len()
        .saturating_add(branch.title.as_ref().map_or(0, String::len))
        .saturating_add(branch.description.as_ref().map_or(0, String::len))
        .saturating_add(json_bytes(&branch.metadata));
    for (name, value) in &branch.o2o {
        total = total.saturating_add(name.len()).saturating_add(value.len());
    }
    for (name, values) in &branch.o2m {
        total = total.saturating_add(name.len());
        for value in values {
            total = total.saturating_add(value.len());
        }
    }
    for reference in &branch.references {
        total = total
            .saturating_add(reference.kind.len())
            .saturating_add(reference.value.len())
            .saturating_add(reference.note.as_ref().map_or(0, String::len));
    }
    total
}

/// Check a single branch against a slot's hard constraints.
pub fn validate_branch(slot: &Slot, branch: &Branch) -> Vec<String> {
    let mut violations = Vec::new();

    if let Some(scope) = &slot.under {
        if !is_under(&branch.path, scope) {
            violations.push(format!("path `{}` is not under `{scope}`", branch.path));
        }
    }

    for (name, value) in &slot.require_o2o {
        match branch.o2o.get(name) {
            Some(actual) if actual == value => {}
            Some(actual) => {
                violations.push(format!("o2o `{name}` is `{actual}`, required `{value}`"))
            }
            None => violations.push(format!("missing required o2o `{name}={value}`")),
        }
    }

    for (name, avoided) in &slot.avoid_o2o {
        for value in avoided {
            if branch.o2o.get(name) == Some(value) {
                violations.push(format!("o2o `{name}={value}` is excluded"));
            }
        }
    }

    for (name, required) in &slot.require_o2m {
        let present = branch.o2m.get(name);
        for v in required {
            if !present.is_some_and(|values| values.contains(v)) {
                violations.push(format!("missing required o2m `{name}={v}`"));
            }
        }
    }

    for (name, avoided) in &slot.avoid_o2m {
        if let Some(values) = branch.o2m.get(name) {
            for v in avoided {
                if values.contains(v) {
                    violations.push(format!("o2m `{name}={v}` is excluded"));
                }
            }
        }
    }

    violations
}

/// Check a packet against a frame: its schema identity and frame binding must
/// match, every selection must satisfy its slot, and each slot must receive
/// exactly its configured `count` of selections.
pub fn validate_packet(frame: &Frame, packet: &SelectionPacket) -> Vec<String> {
    let mut violations = Vec::new();

    if packet.version != PACKET_VERSION {
        violations.push(format!(
            "packet version {} is unsupported; expected {PACKET_VERSION}",
            packet.version
        ));
    }
    if packet.kind != PACKET_KIND {
        violations.push(format!(
            "packet kind `{}` is invalid; expected `{PACKET_KIND}`",
            packet.kind
        ));
    }
    if packet.frame.as_deref() != Some(frame.name.as_str()) {
        violations.push(format!(
            "packet frame is `{}`, expected `{}`",
            packet.frame.as_deref().unwrap_or("<none>"),
            frame.name
        ));
    }

    // A slot's `count` asks for that many *distinct* branches; sampling draws
    // without replacement, so a repeated path within one slot can only come
    // from a hand-edited packet and is a violation even when the count matches.
    let mut seen_per_slot: std::collections::HashSet<(&str, &str)> =
        std::collections::HashSet::new();

    for selection in &packet.selections {
        if let Err(error) = validate_branch_record(&selection.branch) {
            violations.push(format!(
                "selection `{}` contains an invalid branch: {error}",
                selection.branch.path
            ));
        }
        let Some(slot_name) = &selection.slot else {
            violations.push(format!(
                "selection `{}` has no slot for frame `{}`",
                selection.branch.path, frame.name
            ));
            continue;
        };
        if !seen_per_slot.insert((&**slot_name, selection.branch.path.as_str())) {
            violations.push(format!(
                "slot `{slot_name}` selects `{}` more than once",
                selection.branch.path
            ));
        }
        match frame.slot(slot_name) {
            Some(slot) => {
                for reason in validate_branch(slot, &selection.branch) {
                    violations.push(format!("[{slot_name}] {reason}"));
                }
            }
            None => violations.push(format!("selection references unknown slot `{slot_name}`")),
        }
    }

    for slot in &frame.slots {
        let got = packet
            .selections
            .iter()
            .filter(|s| s.slot.as_deref() == Some(slot.name.as_str()))
            .count();
        if got != slot.count {
            violations.push(format!(
                "slot `{}` expects {} selection(s), got {got}",
                slot.name, slot.count
            ));
        }
    }

    violations
}
