use serde::{Deserialize, Serialize};

/// An external reference attached to a branch (file, url, note, markdown, ...).
///
/// In YAML the discriminator field is spelled `type`; we expose it as `kind`
/// in Rust to avoid the reserved keyword.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
