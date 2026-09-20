//! Anonymous candidate proposals, prior to experimental acceptance.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    AtomicMeaning,
    CompositeMeaning,
    Relation,
    GraphMotif,
    SemanticRole,
    Transformation,
    DynamicCoupling,
    LatentAxis,
    CrossDomainStructure,
    WeightRevision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateProposal {
    /// Stored domain-version row ID, not an unversioned domain ID.
    pub domain_version_id: String,
    pub kind: CandidateKind,
    pub value: Map<String, Value>,
}
