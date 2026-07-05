//! Validation for reusable frame definitions.

use std::collections::{BTreeMap, HashSet};

use crate::validate::{
    validate_path, MAX_BRANCH_RECORD_BYTES, MAX_DOMAIN_STRING_BYTES, MAX_FRAME_COLLECTION_ITEMS,
};
use crate::{CoreError, Frame, Result, Slot};

/// Validate frame invariants independently of persistence.
pub fn validate_frame(frame: &Frame) -> Result<()> {
    validate_label("frame name", &frame.name).map_err(|reason| invalid(frame, reason))?;
    if let Some(description) = &frame.description {
        validate_text("frame description", description).map_err(|reason| invalid(frame, reason))?;
    }

    let mut slot_names = HashSet::new();
    let mut items = 0usize;
    let mut bytes = frame
        .name
        .len()
        .saturating_add(frame.description.as_ref().map_or(0, String::len));

    for slot in &frame.slots {
        validate_label("slot name", &slot.name).map_err(|reason| invalid(frame, reason))?;
        if !slot_names.insert(&slot.name) {
            return Err(invalid(
                frame,
                format!("duplicate slot name `{}`", slot.name),
            ));
        }
        if let Some(under) = &slot.under {
            validate_path(under).map_err(|_| {
                invalid(
                    frame,
                    format!("invalid under path `{under}` in slot `{}`", slot.name),
                )
            })?;
        }
        if slot.count == 0 || slot.count > i32::MAX as usize {
            return Err(invalid(
                frame,
                format!("invalid count for slot `{}`", slot.name),
            ));
        }

        validate_o2o("require_o2o", &slot.require_o2o, slot, frame)?;
        validate_o2o("default_o2o", &slot.default_o2o, slot, frame)?;
        // avoid_o2o is a multi-value exclusion, shaped (and validated) like
        // the o2m maps: several avoided values may share one name.
        validate_o2m("avoid_o2o", &slot.avoid_o2o, slot, frame)?;
        validate_o2m("require_o2m", &slot.require_o2m, slot, frame)?;
        validate_o2m("prefer_o2m", &slot.prefer_o2m, slot, frame)?;
        validate_o2m("avoid_o2m", &slot.avoid_o2m, slot, frame)?;

        for (name, required) in &slot.require_o2o {
            if slot
                .default_o2o
                .get(name)
                .is_some_and(|default| default != required)
            {
                return Err(invalid(
                    frame,
                    format!(
                        "default_o2o `{name}` conflicts with its required value in slot `{}`",
                        slot.name
                    ),
                ));
            }
            if slot
                .avoid_o2o
                .get(name)
                .is_some_and(|avoided| avoided.contains(required))
            {
                return Err(invalid(
                    frame,
                    format!(
                        "o2o `{name}={required}` is both required and avoided in slot `{}`",
                        slot.name
                    ),
                ));
            }
        }
        for (name, default) in &slot.default_o2o {
            if slot
                .avoid_o2o
                .get(name)
                .is_some_and(|avoided| avoided.contains(default))
            {
                return Err(invalid(
                    frame,
                    format!(
                        "o2o `{name}={default}` is both a default and avoided in slot `{}`",
                        slot.name
                    ),
                ));
            }
        }
        for (name, required) in &slot.require_o2m {
            if let Some(avoided) = slot.avoid_o2m.get(name) {
                for value in required {
                    if avoided.contains(value) {
                        return Err(invalid(
                            frame,
                            format!(
                                "o2m `{name}={value}` is both required and avoided in slot `{}`",
                                slot.name
                            ),
                        ));
                    }
                }
            }
        }
        for key in &slot.metadata_suggest {
            validate_label("metadata_suggest key", key).map_err(|reason| invalid(frame, reason))?;
        }

        items = items
            .saturating_add(1)
            .saturating_add(o2o_items(&slot.require_o2o))
            .saturating_add(o2o_items(&slot.default_o2o))
            .saturating_add(o2m_items(&slot.avoid_o2o))
            .saturating_add(o2m_items(&slot.require_o2m))
            .saturating_add(o2m_items(&slot.prefer_o2m))
            .saturating_add(o2m_items(&slot.avoid_o2m))
            .saturating_add(slot.metadata_suggest.len());
        if items > MAX_FRAME_COLLECTION_ITEMS {
            return Err(invalid(
                frame,
                format!("frame contains more than {MAX_FRAME_COLLECTION_ITEMS} names and values"),
            ));
        }

        bytes = bytes
            .saturating_add(slot.name.len())
            .saturating_add(slot.under.as_ref().map_or(0, String::len))
            .saturating_add(map_bytes(&slot.require_o2o))
            .saturating_add(map_bytes(&slot.default_o2o))
            .saturating_add(multimap_bytes(&slot.avoid_o2o))
            .saturating_add(multimap_bytes(&slot.require_o2m))
            .saturating_add(multimap_bytes(&slot.prefer_o2m))
            .saturating_add(multimap_bytes(&slot.avoid_o2m))
            .saturating_add(slot.metadata_suggest.iter().map(String::len).sum::<usize>());
        if bytes > MAX_BRANCH_RECORD_BYTES {
            return Err(invalid(
                frame,
                format!("frame exceeds the {MAX_BRANCH_RECORD_BYTES}-byte record limit"),
            ));
        }
    }
    Ok(())
}

fn validate_o2o(
    field: &str,
    values: &BTreeMap<String, String>,
    slot: &Slot,
    frame: &Frame,
) -> Result<()> {
    for (name, value) in values {
        validate_label(&format!("{field} name in slot `{}`", slot.name), name)
            .map_err(|reason| invalid(frame, reason))?;
        validate_label(
            &format!("{field} `{name}` value in slot `{}`", slot.name),
            value,
        )
        .map_err(|reason| invalid(frame, reason))?;
    }
    Ok(())
}

fn validate_o2m(
    field: &str,
    values: &BTreeMap<String, Vec<String>>,
    slot: &Slot,
    frame: &Frame,
) -> Result<()> {
    for (name, entries) in values {
        validate_label(&format!("{field} name in slot `{}`", slot.name), name)
            .map_err(|reason| invalid(frame, reason))?;
        let mut unique = HashSet::new();
        for value in entries {
            validate_label(
                &format!("{field} `{name}` value in slot `{}`", slot.name),
                value,
            )
            .map_err(|reason| invalid(frame, reason))?;
            if !unique.insert(value) {
                return Err(invalid(
                    frame,
                    format!(
                        "{field} `{name}` contains duplicate value `{value}` in slot `{}`",
                        slot.name
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_label(field: &str, value: &str) -> std::result::Result<(), String> {
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    validate_text(field, value)
}

fn validate_text(field: &str, value: &str) -> std::result::Result<(), String> {
    if value.len() > MAX_DOMAIN_STRING_BYTES {
        return Err(format!("{field} is oversized"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(())
}

fn invalid(frame: &Frame, reason: String) -> CoreError {
    CoreError::InvalidFrame {
        name: frame.name.clone(),
        reason,
    }
}

fn o2o_items(values: &BTreeMap<String, String>) -> usize {
    values.len().saturating_mul(2)
}

fn o2m_items(values: &BTreeMap<String, Vec<String>>) -> usize {
    values.iter().fold(0usize, |total, (_, entries)| {
        total.saturating_add(1).saturating_add(entries.len())
    })
}

fn map_bytes(values: &BTreeMap<String, String>) -> usize {
    values.iter().fold(0usize, |total, (name, value)| {
        total.saturating_add(name.len()).saturating_add(value.len())
    })
}

fn multimap_bytes(values: &BTreeMap<String, Vec<String>>) -> usize {
    values.iter().fold(0usize, |total, (name, entries)| {
        entries
            .iter()
            .fold(total.saturating_add(name.len()), |total, value| {
                total.saturating_add(value.len())
            })
    })
}
