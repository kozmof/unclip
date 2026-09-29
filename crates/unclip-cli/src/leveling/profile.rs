//! Reconstruct an `EngineProfile` from a stored run plan.
//!
//! Versions are pinned exactly (`=x.y.z`), not as a caret range: replaying a
//! recorded run against a different build of a plugin must fail loudly rather
//! than silently recalculate with different code.

pub(crate) fn resolved_profile(
    value: &serde_json::Value,
) -> anyhow::Result<(
    unclip_plugin::EngineProfile,
    std::collections::BTreeMap<unclip_epistemic::PluginId, serde_json::Value>,
)> {
    fn section(
        value: &serde_json::Value,
        name: &str,
        params: &mut std::collections::BTreeMap<unclip_epistemic::PluginId, serde_json::Value>,
    ) -> anyhow::Result<Vec<unclip_plugin::PluginSelection>> {
        value
            .get(name)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("stored run plan has no {name} array"))?
            .iter()
            .map(|entry| {
                let id = entry
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("stored {name} entry has no id"))?;
                let version = entry
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("stored {name} entry has no version"))?;
                let id = unclip_epistemic::PluginId::new(id);
                params.insert(
                    id.clone(),
                    entry
                        .get("params")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                );
                Ok(unclip_plugin::PluginSelection {
                    id,
                    version: format!("={version}").parse()?,
                })
            })
            .collect()
    }

    let mut params = std::collections::BTreeMap::new();
    let inferrers = section(value, "inferrers", &mut params)?;
    let sensors = section(value, "sensors", &mut params)?;
    let comparators = section(value, "comparators", &mut params)?;
    let interpreters = if value.get("interpreters").is_some() {
        section(value, "interpreters", &mut params)?
    } else {
        Vec::new()
    };
    let candidate_generators = if value.get("candidate_generators").is_some() {
        section(value, "candidate_generators", &mut params)?
    } else {
        Vec::new()
    };
    let null_models = if value.get("null_models").is_some() {
        section(value, "null_models", &mut params)?
    } else {
        Vec::new()
    };
    Ok((
        unclip_plugin::EngineProfile {
            sensors,
            inferrers,
            comparators,
            interpreters,
            candidate_generators,
            null_models,
        },
        params,
    ))
}
