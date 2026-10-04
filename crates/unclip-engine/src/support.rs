//! Error constructors shared by this crate's plugins and stage operations.
//!
//! These were 38 private copies across the crate's modules, in six different
//! signatures (`&str`, `impl Into<String>`, `impl ToString`, `impl Display`),
//! all with the same body. The duplication was harmless in itself; what it hid
//! was that every one of them produced [`PluginError::Message`], including at
//! the ~25 sites that were reporting *parameter* failures. `unclip-sensors`
//! distinguishes those two cases and states why; this module holds the rest of
//! the workspace to the same line.

use unclip_plugin::PluginError;

/// Report a failed invariant or a malformed input this crate validated itself.
///
/// Takes `impl Display` rather than `impl Into<String>` so an error value can
/// be forwarded directly — several callers pass one straight through
/// `map_err`, and a conversion bound would make each of those spell out
/// `.to_string()`.
pub(crate) fn invalid(message: impl std::fmt::Display) -> PluginError {
    PluginError::Message(message.to_string())
}

/// Report plugin parameters that do not match the plugin's declared schema.
///
/// Bad configuration and a failed calculation are different problems with
/// different fixes, so they get different variants rather than one opaque
/// message. A caller can match [`PluginError::InvalidParams`] to tell "this
/// profile is misconfigured" from "this calculation could not run", which is
/// the distinction the variant exists for.
///
/// This mirrors `unclip_sensors::support::invalid_params`; the two crates
/// cannot share one helper without `unclip-sensors` depending on this one.
pub(crate) fn invalid_params(error: serde_json::Error) -> PluginError {
    PluginError::InvalidParams(error.to_string())
}

/// Hold a plugin to its own declared `params_schema` before invoking it.
///
/// Every family goes through this, sensors included. Sensors were once held to
/// their schema only by `classify_sensor`, which records a violation as a
/// `NotApplicable` reading, so a typo in a profile surfaced as a plausible
/// sparse result rather than an error; `classify_sensor` keeps its check for
/// callers outside the engine. Inferrers, comparators, interpreters, candidate
/// generators and null models all declare a `params_schema`, have it checked
/// for *wellformedness* when they register, and were once never held to it. A
/// schema nothing enforces has the standing of a comment — which is the state
/// `unclip_plugin::schema` was written to end — and it leaves a third-party
/// plugin registered under one of these families answerable to nothing.
///
/// A violation here is an error rather than a recorded sparse reading, for the
/// reason `cross_domain`'s product-sensor check gives: these parameters come
/// from a resolved profile, so a violation is a misconfigured profile or a
/// descriptor that disagrees with the struct behind it, not thin evidence.
///
/// [`Sensor`]: unclip_plugin::Sensor
pub(crate) fn require_declared_params(
    plugin: &unclip_epistemic::PluginId,
    schema: &str,
    params: &serde_json::Value,
) -> unclip_plugin::Result<()> {
    unclip_plugin::validate_params(schema, params).map_err(|violation| {
        PluginError::InvalidParams(format!(
            "{plugin} parameters do not satisfy the declared schema: {violation}"
        ))
    })
}

/// Reject a run id that cannot form a usable [`DerivedId`].
///
/// Every stage mints its emitted ids as `{run_id}/{plugin_id}`, so a blank run
/// id silently produces ids like `/sensor.coverage` — indistinguishable between
/// runs and unusable as a provenance key.
///
/// This is one function because it was three different rules for one
/// invariant: `measure` rejected a whitespace-only id, `interpret` rejected
/// only a strictly empty one, and `infer` — which mints ids exactly the same
/// way — checked nothing at all. The stage name is a parameter so the message
/// still says which stage refused.
///
/// [`DerivedId`]: unclip_epistemic::DerivedId
///
/// `#` is rejected as well, because it marks a token's later emissions: a run
/// id `r#1` would mint `r#1/p`, which reads as an emission of run `r`. `/` is
/// allowed, since stages nest sub-runs as `{run_id}/before`; the registry keeps
/// plugin ids free of `/`, so `{run_id}/{plugin_id}` still splits one way.
pub(crate) fn require_run_id(stage: &str, id: &str) -> unclip_plugin::Result<()> {
    if id.trim().is_empty() {
        return Err(invalid(format!("{stage} requires a non-empty run ID")));
    }
    if id.contains('#') {
        return Err(invalid(format!(
            "{stage} run ID {id:?} must not contain '#', which derived IDs reserve"
        )));
    }
    Ok(())
}

/// Reject a value whose provenance names a plugin other than the one invoked.
///
/// A plugin can mint its own emit token instead of using the one it was handed,
/// so every family's output is checked here, where `verify` passes too.
pub(crate) fn require_producer<T: ?Sized, O: unclip_epistemic::OperationKind>(
    plugin: &unclip_epistemic::PluginId,
    value: &unclip_epistemic::Derived<T, O>,
) -> unclip_plugin::Result<()> {
    let producer = &value.provenance().producer;
    if producer == plugin {
        return Ok(());
    }
    Err(PluginError::ForeignProducer {
        plugin: plugin.clone(),
        value: value.id().clone(),
        producer: producer.clone(),
    })
}

/// Take the map produced by an object-shaped JSON literal without copying it.
pub(crate) fn json_object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(object) => object,
        _ => unreachable!("expected an object-shaped JSON literal"),
    }
}
