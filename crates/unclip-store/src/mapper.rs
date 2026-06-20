//! Conversions between SeaORM entity rows and the domain `Branch`.
//!
//! Keeping this in one place is what lets application logic depend only on
//! `unclip_core` types and never touch SeaORM directly.

use std::collections::BTreeMap;

use sea_orm::ActiveValue::{NotSet, Set};
use unclip_core::{parent_of, Branch, Reference};
use unclip_entity::{branch_o2m_values, branch_o2o_values, branch_references, branches};

/// Build a `branches` active model for insertion/update.
///
/// `id` is left unset when `None` so SQLite assigns it.
pub fn branch_active_model(branch: &Branch, created_at: &str, updated_at: &str) -> branches::ActiveModel {
    let metadata_json = if branch.metadata.is_null() {
        None
    } else {
        Some(branch.metadata.to_string())
    };

    branches::ActiveModel {
        id: match branch.id {
            Some(id) => Set(id as i32),
            None => NotSet,
        },
        path: Set(branch.path.clone()),
        parent_path: Set(parent_of(&branch.path)),
        title: Set(branch.title.clone()),
        description: Set(branch.description.clone()),
        weight: Set(branch.weight),
        metadata_json: Set(metadata_json),
        created_at: Set(created_at.to_string()),
        updated_at: Set(updated_at.to_string()),
    }
}

/// o2o active-model rows for a branch.
pub fn o2o_active_models(branch_id: i32, branch: &Branch) -> Vec<branch_o2o_values::ActiveModel> {
    branch
        .o2o
        .iter()
        .map(|(name, value)| branch_o2o_values::ActiveModel {
            branch_id: Set(branch_id),
            name: Set(name.clone()),
            value: Set(value.clone()),
        })
        .collect()
}

/// o2m active-model rows for a branch.
pub fn o2m_active_models(branch_id: i32, branch: &Branch) -> Vec<branch_o2m_values::ActiveModel> {
    branch
        .o2m
        .iter()
        .flat_map(|(name, values)| {
            values.iter().map(move |value| branch_o2m_values::ActiveModel {
                branch_id: Set(branch_id),
                name: Set(name.clone()),
                value: Set(value.clone()),
            })
        })
        .collect()
}

/// Reference active-model rows for a branch.
pub fn reference_active_models(
    branch_id: i32,
    branch: &Branch,
) -> Vec<branch_references::ActiveModel> {
    branch
        .references
        .iter()
        .map(|r| branch_references::ActiveModel {
            id: NotSet,
            branch_id: Set(branch_id),
            r#type: Set(r.kind.clone()),
            value: Set(r.value.clone()),
            note: Set(r.note.clone()),
        })
        .collect()
}

/// Assemble a domain `Branch` from a row and its loaded child rows.
pub fn assemble_branch(
    model: branches::Model,
    o2o: Vec<branch_o2o_values::Model>,
    o2m: Vec<branch_o2m_values::Model>,
    refs: Vec<branch_references::Model>,
) -> anyhow::Result<Branch> {
    let mut o2o_map = BTreeMap::new();
    for row in o2o {
        o2o_map.insert(row.name, row.value);
    }

    // o2m values are a set (DRAFT §6); return them in a deterministic order.
    let mut o2m_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in o2m {
        o2m_map.entry(row.name).or_default().push(row.value);
    }
    for values in o2m_map.values_mut() {
        values.sort();
    }

    let metadata = match model.metadata_json {
        Some(ref s) if !s.is_empty() => serde_json::from_str(s)?,
        _ => serde_json::Value::Null,
    };

    let references = refs
        .into_iter()
        .map(|r| Reference {
            kind: r.r#type,
            value: r.value,
            note: r.note,
        })
        .collect();

    Ok(Branch {
        id: Some(model.id as i64),
        path: model.path,
        title: model.title,
        description: model.description,
        o2o: o2o_map,
        o2m: o2m_map,
        weight: model.weight,
        metadata,
        references,
    })
}
