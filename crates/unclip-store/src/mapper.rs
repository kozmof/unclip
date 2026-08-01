//! Conversions between SeaORM entity rows and the domain `Branch`.
//!
//! Keeping this in one place is what lets application logic depend only on
//! `unclip_core` types and never touch SeaORM directly.

use std::collections::{BTreeMap, HashSet};

use sea_orm::ActiveValue::{NotSet, Set};
use unclip_core::{parent_of, Branch, Reference};
use unclip_entity::{branch_o2m_values, branch_o2o_values, branch_references, branches};

/// The child-table collections of a branch, split off from its parent row.
///
/// [`into_branch_active_model`] hands these back so a caller that already owns
/// its branch can move every indexed value into the child rows instead of
/// copying it out of a borrow.
pub struct BranchChildren {
    pub o2o: BTreeMap<String, String>,
    pub o2m: BTreeMap<String, Vec<String>>,
    pub references: Vec<Reference>,
}

/// Owned counterpart of [`branch_active_model`].
///
/// `update` and `upsert_many` are handed the branch they write, so the scalar
/// fields move into the row rather than being cloned. The child collections
/// come back untouched for [`into_o2o_active_models`] and its siblings, which
/// is the whole point: an import of N branches would otherwise copy every
/// path, title, o2o/o2m name and value, and reference on the way to SQL, only
/// to drop the originals at the end of the loop.
///
/// [`branch_active_model`] stays for `add`, which genuinely only has a borrow.
pub fn into_branch_active_model(
    branch: Branch,
    created_at: &str,
    updated_at: &str,
) -> (branches::ActiveModel, BranchChildren) {
    let Branch {
        id,
        path,
        title,
        description,
        o2o,
        o2m,
        weight,
        metadata,
        references,
        ..
    } = branch;

    let metadata_json = if metadata.is_null() {
        None
    } else {
        Some(metadata.to_string())
    };
    // Borrows `path`, so it is taken (and owned) before `path` moves into the
    // row below.
    let parent_path = parent_of(&path).map(str::to_string);

    let model = branches::ActiveModel {
        id: match id {
            Some(id) => Set(id),
            None => NotSet,
        },
        path: Set(path),
        parent_path: Set(parent_path),
        title: Set(title),
        description: Set(description),
        weight: Set(weight),
        metadata_json: Set(metadata_json),
        created_at: Set(created_at.to_string()),
        updated_at: Set(updated_at.to_string()),
    };

    (
        model,
        BranchChildren {
            o2o,
            o2m,
            references,
        },
    )
}

/// Owned counterpart of [`o2o_active_models`]. o2o is one-to-one, so each
/// entry becomes exactly one row and both its name and value move into it.
pub fn into_o2o_active_models(
    branch_id: i64,
    o2o: BTreeMap<String, String>,
) -> Vec<branch_o2o_values::ActiveModel> {
    o2o.into_iter()
        .map(|(name, value)| branch_o2o_values::ActiveModel {
            branch_id: Set(branch_id),
            name: Set(name),
            value: Set(value),
        })
        .collect()
}

/// Owned counterpart of [`o2m_active_models`], with the same duplicate-dropping
/// contract.
///
/// Duplicates are removed by sorting rather than by probing a `HashSet`: owning
/// the values means a set probe would have to copy each one to insert it, which
/// is the cost this function exists to avoid. Sorting is free of that, and the
/// resulting order is not observable — o2m is a set, and [`assemble_branch`]
/// sorts each name's values on the way back out.
///
/// One row per value still needs one owned name per row, so the name is cloned
/// for every row but the last, which takes the name itself.
pub fn into_o2m_active_models(
    branch_id: i64,
    o2m: BTreeMap<String, Vec<String>>,
) -> Vec<branch_o2m_values::ActiveModel> {
    let mut rows = Vec::new();
    for (name, mut values) in o2m {
        values.sort();
        values.dedup();
        let Some(last) = values.pop() else {
            continue;
        };
        for value in values {
            rows.push(o2m_row(branch_id, name.clone(), value));
        }
        rows.push(o2m_row(branch_id, name, last));
    }
    rows
}

fn o2m_row(branch_id: i64, name: String, value: String) -> branch_o2m_values::ActiveModel {
    branch_o2m_values::ActiveModel {
        branch_id: Set(branch_id),
        name: Set(name),
        value: Set(value),
    }
}

/// Owned counterpart of [`reference_active_models`].
pub fn into_reference_active_models(
    branch_id: i64,
    references: Vec<Reference>,
) -> Vec<branch_references::ActiveModel> {
    references
        .into_iter()
        .map(|r| branch_references::ActiveModel {
            id: NotSet,
            branch_id: Set(branch_id),
            r#type: Set(r.kind),
            value: Set(r.value),
            note: Set(r.note),
        })
        .collect()
}

/// Build a `branches` active model for insertion/update.
///
/// `id` is left unset when `None` so SQLite assigns it. Infallible: the domain
/// and storage id widths now match, so there is nothing left here that can fail.
///
/// For a caller that owns its branch, prefer [`into_branch_active_model`].
pub fn branch_active_model(
    branch: &Branch,
    created_at: &str,
    updated_at: &str,
) -> branches::ActiveModel {
    let metadata_json = if branch.metadata.is_null() {
        None
    } else {
        Some(branch.metadata.to_string())
    };

    branches::ActiveModel {
        id: match branch.id {
            Some(id) => Set(id),
            None => NotSet,
        },
        path: Set(branch.path.clone()),
        parent_path: Set(parent_of(&branch.path).map(str::to_string)),
        title: Set(branch.title.clone()),
        description: Set(branch.description.clone()),
        weight: Set(branch.weight),
        metadata_json: Set(metadata_json),
        created_at: Set(created_at.to_string()),
        updated_at: Set(updated_at.to_string()),
    }
}

/// o2o active-model rows for a branch.
pub fn o2o_active_models(branch_id: i64, branch: &Branch) -> Vec<branch_o2o_values::ActiveModel> {
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
///
/// o2m is a set: duplicate values under one name are dropped here so an
/// imported branch carrying e.g. `topic: [locker, locker]` cannot violate the
/// `(branch_id, name, value)` primary key.
pub fn o2m_active_models(branch_id: i64, branch: &Branch) -> Vec<branch_o2m_values::ActiveModel> {
    let mut rows = Vec::new();
    for (name, values) in &branch.o2m {
        let mut seen = HashSet::new();
        for value in values {
            if seen.insert(value) {
                rows.push(branch_o2m_values::ActiveModel {
                    branch_id: Set(branch_id),
                    name: Set(name.clone()),
                    value: Set(value.clone()),
                });
            }
        }
    }
    rows
}

/// Reference active-model rows for a branch.
pub fn reference_active_models(
    branch_id: i64,
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

    // o2m values are a set; return them in a deterministic order.
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
        id: Some(model.id),
        revision: Some(model.updated_at),
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
