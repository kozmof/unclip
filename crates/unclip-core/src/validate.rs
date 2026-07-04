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

/// Validate a branch path address.
///
/// A path must be absolute (`/`-prefixed), have no empty segments (no `//`),
/// no trailing slash, and contain no whitespace. The bare root `/` is not a
/// valid branch address.
pub fn validate_path(path: &str) -> Result<()> {
    let invalid = |path: &str| CoreError::InvalidPath(path.to_string());

    if path.len() > MAX_PATH_BYTES || path == "/" || !path.starts_with('/') || path.ends_with('/') {
        return Err(invalid(path));
    }
    for segment in path.split('/').skip(1) {
        if segment.is_empty()
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
        if name.is_empty()
            || name.len() > MAX_DOMAIN_STRING_BYTES
            || name.chars().any(char::is_control)
        {
            return Err(invalid(
                "o2o name must not be empty or contain control characters".to_string(),
            ));
        }
        if value.is_empty()
            || value.len() > MAX_DOMAIN_STRING_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(invalid(format!(
                "o2o `{name}` value must not be empty or contain control characters"
            )));
        }
    }

    for (name, values) in &branch.o2m {
        if name.is_empty()
            || name.len() > MAX_DOMAIN_STRING_BYTES
            || name.chars().any(char::is_control)
        {
            return Err(invalid(
                "o2m name must not be empty or contain control characters".to_string(),
            ));
        }
        for value in values {
            if value.is_empty()
                || value.len() > MAX_DOMAIN_STRING_BYTES
                || value.chars().any(char::is_control)
            {
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
    if reference.kind.is_empty()
        || reference.kind.len() > MAX_DOMAIN_STRING_BYTES
        || reference.kind.chars().any(char::is_control)
    {
        return Err(CoreError::InvalidBranch {
            path: "<reference>".to_string(),
            reason: "reference type must not be empty or contain control characters".to_string(),
        });
    }
    if reference.value.is_empty()
        || reference.value.len() > MAX_DOMAIN_STRING_BYTES
        || reference.value.chars().any(char::is_control)
    {
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

fn branch_record_bytes(branch: &Branch) -> usize {
    let mut total = branch
        .path
        .len()
        .saturating_add(branch.title.as_ref().map_or(0, String::len))
        .saturating_add(branch.description.as_ref().map_or(0, String::len))
        .saturating_add(branch.metadata.to_string().len());
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

    for (name, value) in &slot.avoid_o2o {
        if branch.o2o.get(name) == Some(value) {
            violations.push(format!("o2o `{name}={value}` is excluded"));
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
