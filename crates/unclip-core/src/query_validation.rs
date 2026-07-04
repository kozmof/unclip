//! Validation and resource bounds for repository queries.

use std::collections::BTreeMap;

use crate::validate::{validate_path, MAX_DOMAIN_STRING_BYTES, MAX_QUERY_FILTER_ITEMS};
use crate::{CoreError, Result, SampleQuery};

/// Validate query fields and cap their aggregate SQL complexity.
pub fn validate_sample_query(query: &SampleQuery) -> Result<()> {
    if let Some(under) = &query.under {
        validate_path(under).map_err(|_| {
            CoreError::InvalidQuery("scope must be a valid absolute branch path".to_string())
        })?;
    }

    validate_o2o(&query.require_o2o)?;
    for values in [
        &query.avoid_o2o,
        &query.require_o2m,
        &query.prefer_o2m,
        &query.avoid_o2m,
    ] {
        validate_o2m(values)?;
    }

    let items = usize::from(query.under.is_some())
        .saturating_add(query.require_o2o.len().saturating_mul(2))
        .saturating_add(multimap_items(&query.avoid_o2o))
        .saturating_add(multimap_items(&query.require_o2m).saturating_mul(2))
        .saturating_add(multimap_items(&query.prefer_o2m))
        .saturating_add(multimap_items(&query.avoid_o2m));
    if items > MAX_QUERY_FILTER_ITEMS {
        return Err(CoreError::InvalidQuery(format!(
            "query contains more than {MAX_QUERY_FILTER_ITEMS} filter names and values"
        )));
    }
    Ok(())
}

fn validate_o2o(values: &BTreeMap<String, String>) -> Result<()> {
    for (name, value) in values {
        validate_part("o2o name", name)?;
        validate_part("o2o value", value)?;
    }
    Ok(())
}

fn validate_o2m(values: &BTreeMap<String, Vec<String>>) -> Result<()> {
    for (name, entries) in values {
        validate_part("filter name", name)?;
        for value in entries {
            validate_part("filter value", value)?;
        }
    }
    Ok(())
}

fn validate_part(field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(CoreError::InvalidQuery(format!(
            "{field} must not be empty"
        )));
    }
    if value.len() > MAX_DOMAIN_STRING_BYTES {
        return Err(CoreError::InvalidQuery(format!("{field} is oversized")));
    }
    if value.chars().any(char::is_control) {
        return Err(CoreError::InvalidQuery(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(())
}

fn multimap_items(values: &BTreeMap<String, Vec<String>>) -> usize {
    values.iter().fold(0usize, |total, (_, entries)| {
        total.saturating_add(1).saturating_add(entries.len())
    })
}
