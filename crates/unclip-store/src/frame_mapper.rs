//! Conversions between SeaORM frame rows and the domain `Frame`/`Slot`.

use std::collections::BTreeMap;

use unclip_core::{Frame, Slot};
use unclip_entity::{frame_slot_o2m_values, frame_slot_o2o_values, frame_slots};

/// o2o mode discriminators stored in `frame_slot_o2o_values.mode`.
pub const O2O_REQUIRE: &str = "require";
pub const O2O_DEFAULT: &str = "default";
pub const O2O_AVOID: &str = "avoid";

/// o2m mode discriminators stored in `frame_slot_o2m_values.mode`.
pub const O2M_REQUIRE: &str = "require";
pub const O2M_PREFER: &str = "prefer";
pub const O2M_AVOID: &str = "avoid";

/// Assemble a domain `Slot` from its row plus its value rows.
pub fn assemble_slot(
    model: frame_slots::Model,
    o2o: Vec<frame_slot_o2o_values::Model>,
    o2m: Vec<frame_slot_o2m_values::Model>,
) -> anyhow::Result<Slot> {
    let mut require_o2o = BTreeMap::new();
    let mut default_o2o = BTreeMap::new();
    let mut avoid_o2o = BTreeMap::new();
    for row in o2o {
        let target = match row.mode.as_str() {
            O2O_REQUIRE => &mut require_o2o,
            O2O_DEFAULT => &mut default_o2o,
            O2O_AVOID => &mut avoid_o2o,
            other => anyhow::bail!("unknown o2o slot mode `{other}`"),
        };
        target.insert(row.name, row.value);
    }

    let mut require_o2m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut prefer_o2m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut avoid_o2m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in o2m {
        let target = match row.mode.as_str() {
            O2M_REQUIRE => &mut require_o2m,
            O2M_PREFER => &mut prefer_o2m,
            O2M_AVOID => &mut avoid_o2m,
            other => anyhow::bail!("unknown o2m slot mode `{other}`"),
        };
        target.entry(row.name).or_default().push(row.value);
    }
    for values in require_o2m
        .values_mut()
        .chain(prefer_o2m.values_mut())
        .chain(avoid_o2m.values_mut())
    {
        values.sort();
    }

    let metadata_suggest = match model.metadata_suggest_json {
        Some(ref s) if !s.is_empty() => serde_json::from_str(s)?,
        _ => Vec::new(),
    };

    Ok(Slot {
        name: model.name,
        under: model.under_path,
        require_o2o,
        default_o2o,
        avoid_o2o,
        require_o2m,
        prefer_o2m,
        avoid_o2m,
        count: model.count.max(0) as usize,
        avoid_recent: model.avoid_recent != 0,
        weighted: model.weighted != 0,
        metadata_suggest,
    })
}

/// Flatten a slot's o2o maps into `(mode, name, value)` rows.
pub fn slot_o2o_rows(slot: &Slot) -> Vec<(&'static str, String, String)> {
    let mut rows = Vec::new();
    for (name, value) in &slot.require_o2o {
        rows.push((O2O_REQUIRE, name.clone(), value.clone()));
    }
    for (name, value) in &slot.default_o2o {
        rows.push((O2O_DEFAULT, name.clone(), value.clone()));
    }
    for (name, value) in &slot.avoid_o2o {
        rows.push((O2O_AVOID, name.clone(), value.clone()));
    }
    rows
}

/// Flatten a slot's o2m maps into `(mode, name, value)` rows.
pub fn slot_o2m_rows(slot: &Slot) -> Vec<(&'static str, String, String)> {
    let mut rows = Vec::new();
    for (name, values) in &slot.require_o2m {
        for value in values {
            rows.push((O2M_REQUIRE, name.clone(), value.clone()));
        }
    }
    for (name, values) in &slot.prefer_o2m {
        for value in values {
            rows.push((O2M_PREFER, name.clone(), value.clone()));
        }
    }
    for (name, values) in &slot.avoid_o2m {
        for value in values {
            rows.push((O2M_AVOID, name.clone(), value.clone()));
        }
    }
    rows
}

/// Serialize `metadata_suggest` to JSON, or `None` when empty.
pub fn metadata_suggest_json(slot: &Slot) -> anyhow::Result<Option<String>> {
    if slot.metadata_suggest.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::to_string(&slot.metadata_suggest)?))
    }
}

/// Assemble a domain `Frame` from its name/description and slots.
pub fn assemble_frame(name: String, description: Option<String>, slots: Vec<Slot>) -> Frame {
    Frame {
        name,
        description,
        slots,
    }
}
