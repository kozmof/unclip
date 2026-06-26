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
use crate::{Branch, Reference};

/// Validate a branch path address.
///
/// A path must be absolute (`/`-prefixed), have no empty segments (no `//`),
/// no trailing slash, and contain no whitespace. The bare root `/` is not a
/// valid branch address.
pub fn validate_path(path: &str) -> Result<()> {
    let invalid = |path: &str| CoreError::InvalidPath(path.to_string());

    if path == "/" || !path.starts_with('/') || path.ends_with('/') {
        return Err(invalid(path));
    }
    for segment in path.split('/').skip(1) {
        if segment.is_empty() || segment.chars().any(char::is_whitespace) {
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

    for (name, value) in &branch.o2o {
        if name.is_empty() {
            return Err(invalid("o2o name must not be empty".to_string()));
        }
        if value.is_empty() {
            return Err(invalid(format!("o2o `{name}` value must not be empty")));
        }
    }

    for (name, values) in &branch.o2m {
        if name.is_empty() {
            return Err(invalid("o2m name must not be empty".to_string()));
        }
        for value in values {
            if value.is_empty() {
                return Err(invalid(format!("o2m `{name}` value must not be empty")));
            }
        }
    }

    for reference in &branch.references {
        validate_reference(reference).map_err(|err| invalid(err.to_string()))?;
    }

    Ok(())
}

/// Validate a reference before storing it independently of a full branch.
pub fn validate_reference(reference: &Reference) -> Result<()> {
    if reference.kind.is_empty() {
        return Err(CoreError::InvalidBranch {
            path: "<reference>".to_string(),
            reason: "reference type must not be empty".to_string(),
        });
    }
    if reference.value.is_empty() {
        return Err(CoreError::InvalidBranch {
            path: "<reference>".to_string(),
            reason: "reference value must not be empty".to_string(),
        });
    }
    Ok(())
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

/// Check a packet against a frame: every selection must satisfy its slot, and
/// each slot must receive at least its required `count` of selections.
pub fn validate_packet(frame: &Frame, packet: &SelectionPacket) -> Vec<String> {
    let mut violations = Vec::new();

    for selection in &packet.selections {
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
        if got < slot.count {
            violations.push(format!(
                "slot `{}` expects {} selection(s), got {got}",
                slot.name, slot.count
            ));
        }
    }

    violations
}
