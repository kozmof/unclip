//! Persistable run and experiment records.
//!
//! These types are the contract between the calculation engine, which produces
//! run records, and the store, which writes them to SQLite. They live in their
//! own crate so that contract does not force one side to depend on the other:
//! `unclip-engine` performs no persistence, and routing its record types
//! through `unclip-store` used to make every engine consumer link SeaORM,
//! SQLx and SQLite to run a calculation.
//!
//! Nothing here knows how a record is stored. The types carry timestamps and
//! foreign keys as plain strings because that is what the schema holds; mapping
//! them to and from rows is `unclip-store`'s job.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use unclip_epistemic::{Calculated, DerivedId, ParameterHash, PluginId};
use unclip_measure::Delta;
use unclip_observe::{Alignment, Observation, ObservationId, PartialRanking};

/// How far a planned engine run has progressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineRunStatus {
    Planned,
    Running,
    Completed,
    Failed,
}

/// A stored status string that names no known state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown engine run status: {0}")]
pub struct UnknownEngineRunStatus(pub String);

impl EngineRunStatus {
    /// The stored spelling. Persisted rows carry these exact strings, so the
    /// mapping lives with the type rather than in whichever layer writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for EngineRunStatus {
    type Err = UnknownEngineRunStatus;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "planned" => Ok(Self::Planned),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(UnknownEngineRunStatus(other.to_owned())),
        }
    }
}

impl std::fmt::Display for EngineRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One planned or completed engine run, with the plugin versions it resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineRunRecord {
    pub id: String,
    pub resolved_plan: serde_json::Value,
    pub status: EngineRunStatus,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub metadata: serde_json::Value,
}

/// One sensor invocation within an engine run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorRunRecord {
    pub id: String,
    pub engine_run_id: String,
    pub sensor: PluginId,
    pub sensor_version: semver::Version,
    pub params: serde_json::Value,
    pub params_hash: ParameterHash,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// An inference product paired with the provenance identity it was stored under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedInference<T> {
    pub provenance: DerivedId,
    pub value: T,
}

/// Exact inference products selected by a measurement-only run. Keeping their
/// values and provenance identities prevents later alignments or rankings from
/// silently changing replay inputs. Sequence order is preserved as recorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementInputSnapshot {
    pub observations: Vec<RecordedInference<Observation>>,
    pub alignments: Vec<RecordedInference<Alignment>>,
    pub rankings: Vec<RecordedInference<PartialRanking>>,
}

/// Everything a recorded run needs to re-execute its calculation stages.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineRunReplay {
    pub run: EngineRunRecord,
    pub sensor_runs: Vec<SensorRunRecord>,
    pub provenance_ids: Vec<String>,
    pub profile_ids: Vec<String>,
    pub observations: Vec<RecordedInference<Observation>>,
    pub alignments: Vec<RecordedInference<Alignment>>,
    pub rankings: Vec<RecordedInference<PartialRanking>>,
}

/// The result of testing one candidate against held-out evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentOutcome {
    pub candidate_id: DerivedId,
    pub domain_version_id: String,
    pub frame_version_id: String,
    pub plan: serde_json::Map<String, serde_json::Value>,
    pub result: serde_json::Map<String, serde_json::Value>,
    /// Explicit sequence order is preserved independently in each split.
    pub training: Vec<ObservationId>,
    pub held_out: Vec<ObservationId>,
    pub started_at: String,
}

/// One before/after profile comparison recorded for an experiment.
#[derive(Debug, Clone, PartialEq)]
pub struct ExperimentDelta {
    pub before_profile_id: String,
    pub after_profile_id: String,
    pub calculated: Calculated<Delta>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_run_status_round_trips_through_its_stored_spelling() {
        for status in [
            EngineRunStatus::Planned,
            EngineRunStatus::Running,
            EngineRunStatus::Completed,
            EngineRunStatus::Failed,
        ] {
            assert_eq!(status.as_str().parse::<EngineRunStatus>(), Ok(status));
            assert_eq!(status.to_string(), status.as_str());
        }
    }

    #[test]
    fn an_unrecognized_status_names_what_it_read() {
        assert_eq!(
            "abandoned".parse::<EngineRunStatus>(),
            Err(UnknownEngineRunStatus("abandoned".to_owned()))
        );
    }
}
