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
