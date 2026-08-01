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
    // avoid is a multi-value exclusion: several avoided values per name.
    let mut avoid_o2o: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in o2o {
        let target = match row.mode.as_str() {
            O2O_REQUIRE => &mut require_o2o,
            O2O_DEFAULT => &mut default_o2o,
            O2O_AVOID => {
                avoid_o2o.entry(row.name).or_default().push(row.value);
                continue;
            }
            other => anyhow::bail!("unknown o2o slot mode `{other}`"),
        };
        target.insert(row.name, row.value);
    }
    for values in avoid_o2o.values_mut() {
        values.sort();
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

/// A flattened slot constraint: `(mode, name, value)`.
pub type ValueRow = (&'static str, String, String);

/// The constraint maps of a slot, split off from its row.
///
/// Mirrors [`crate::mapper::BranchChildren`]: `save_frame_in_txn` is handed the
/// frame it writes, so [`SlotValues::into_rows`] moves every name and value
/// into the value-table rows instead of copying them out of a borrow.
pub struct SlotValues {
    pub require_o2o: BTreeMap<String, String>,
    pub default_o2o: BTreeMap<String, String>,
    pub avoid_o2o: BTreeMap<String, Vec<String>>,
    pub require_o2m: BTreeMap<String, Vec<String>>,
    pub prefer_o2m: BTreeMap<String, Vec<String>>,
    pub avoid_o2m: BTreeMap<String, Vec<String>>,
}

impl SlotValues {
    /// Flatten into `(o2o rows, o2m rows)`, consuming the maps.
    ///
    /// Both halves are returned from one call so the two row sets cannot be
    /// built from mismatched slots.
    pub fn into_rows(self) -> (Vec<ValueRow>, Vec<ValueRow>) {
        let mut o2o = Vec::new();
        for (name, value) in self.require_o2o {
            o2o.push((O2O_REQUIRE, name, value));
        }
        for (name, value) in self.default_o2o {
            o2o.push((O2O_DEFAULT, name, value));
        }
        push_multi(&mut o2o, O2O_AVOID, self.avoid_o2o);

        let mut o2m = Vec::new();
        push_multi(&mut o2m, O2M_REQUIRE, self.require_o2m);
        push_multi(&mut o2m, O2M_PREFER, self.prefer_o2m);
        push_multi(&mut o2m, O2M_AVOID, self.avoid_o2m);

        (o2o, o2m)
    }
}

/// Flatten one multi-value constraint map under a single mode. Each row needs
/// its own owned name, so the name is cloned for every row but the last, which
/// takes the name itself.
fn push_multi(rows: &mut Vec<ValueRow>, mode: &'static str, map: BTreeMap<String, Vec<String>>) {
    for (name, mut values) in map {
        let Some(last) = values.pop() else {
            continue;
        };
        for value in values {
            rows.push((mode, name.clone(), value));
        }
        rows.push((mode, name, last));
    }
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
