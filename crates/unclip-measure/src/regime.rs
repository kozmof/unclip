//! Anonymous partitions of an explicitly ordered observation sequence.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{ObservationSequence, OrderedObservation};

/// A partition at explicitly selected observation boundaries. This records
/// intervals, not a claim that their contents are stationary or semantically
/// distinct. Boundaries can be selected from change-point evidence by a harness
/// that records that evidence and the selection parameters in provenance.
/// The validated data is shared, for the reason [`ObservationSequence`] is:
/// a partition carries one entry per observation and is reported as evidence.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RegimeData")]
pub struct RegimePartition(Arc<RegimeData>);

// Written out rather than declared with `into = "RegimeData"`, which serialized
// `self.clone()` — copying the starts and, through the inner sequence's own
// conversion, every coordinate again.
impl Serialize for RegimePartition {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegimeData {
    sequence: ObservationSequence,
    starts: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("regime partition requires a nonempty sequence and increasing in-range starts beginning at zero")]
pub struct InvalidRegimePartition;

impl RegimePartition {
    /// Starts are zero-based sequence indexes, including zero. Every observation
    /// belongs to exactly one interval; gaps in sequence coordinates are retained.
    pub fn new(
        sequence: ObservationSequence,
        starts: Vec<usize>,
    ) -> Result<Self, InvalidRegimePartition> {
        Self::try_from(RegimeData { sequence, starts })
    }

    pub fn sequence(&self) -> &ObservationSequence {
        &self.0.sequence
    }
    pub fn starts(&self) -> &[usize] {
        &self.0.starts
    }

    pub fn intervals(&self) -> impl Iterator<Item = &[OrderedObservation]> {
        self.0.starts.iter().enumerate().map(|(index, &start)| {
            let end = self
                .0
                .starts
                .get(index + 1)
                .copied()
                .unwrap_or(self.0.sequence.observations().len());
            &self.0.sequence.observations()[start..end]
        })
    }
}

impl TryFrom<RegimeData> for RegimePartition {
    type Error = InvalidRegimePartition;
    fn try_from(value: RegimeData) -> Result<Self, Self::Error> {
        let count = value.sequence.observations().len();
        if count == 0
            || value.starts.first() != Some(&0)
            || value.starts.iter().any(|start| *start >= count)
            || value.starts.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(InvalidRegimePartition);
        }
        Ok(Self(Arc::new(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unclip_observe::ObservationId;

    fn sequence() -> ObservationSequence {
        ObservationSequence::new(
            (0..4)
                .map(|i| OrderedObservation {
                    observation: ObservationId::new(i.to_string()),
                    position: i * 10,
                })
                .collect(),
        )
        .unwrap()
    }

    /// A partition holds one entry per observation and is reported as evidence,
    /// so copying one shares it and writing one borrows it. The owning shape it
    /// used to declare serialized `self.clone()`, which copied the starts and —
    /// through the inner sequence's own conversion — every coordinate again;
    /// the bytes were the same either way, so only pointer identity catches a
    /// regression back to it.
    #[test]
    fn copying_a_partition_or_its_sequence_shares_one_allocation() {
        let sequence = sequence();
        let copied = sequence.clone();
        assert_eq!(sequence, copied);
        assert!(std::ptr::eq(sequence.observations(), copied.observations()));

        let partition = RegimePartition::new(sequence.clone(), vec![0, 2]).unwrap();
        let copied = partition.clone();
        assert_eq!(partition, copied);
        assert!(std::ptr::eq(partition.starts(), copied.starts()));
        assert!(std::ptr::eq(
            partition.sequence().observations(),
            copied.sequence().observations()
        ));
        // Building a partition shares the sequence it was given rather than
        // copying its coordinates in.
        assert!(std::ptr::eq(
            partition.sequence().observations(),
            sequence.observations()
        ));
        // Writing one is unchanged, and still round-trips into the same value.
        let json = serde_json::to_string(&partition).unwrap();
        assert_eq!(
            json,
            serde_json::to_string(&serde_json::json!({
                "sequence": sequence,
                "starts": [0, 2],
            }))
            .unwrap()
        );
        assert_eq!(
            serde_json::from_str::<RegimePartition>(&json).unwrap(),
            partition
        );
    }

    #[test]
    fn intervals_cover_every_observation_once_without_labels() {
        let sequence = sequence();
        let partition = RegimePartition::new(sequence.clone(), vec![0, 2, 3]).unwrap();
        assert_eq!(partition.starts(), &[0, 2, 3]);
        assert_eq!(partition.sequence(), &sequence);
        assert_eq!(
            partition.intervals().map(<[_]>::len).collect::<Vec<_>>(),
            vec![2, 1, 1]
        );
        assert_eq!(
            partition.intervals().flatten().cloned().collect::<Vec<_>>(),
            sequence.observations()
        );
        let json = serde_json::to_string(&partition).unwrap();
        assert_eq!(
            serde_json::from_str::<RegimePartition>(&json).unwrap(),
            partition
        );
        assert_eq!(
            RegimePartition::new(sequence, vec![0])
                .unwrap()
                .intervals()
                .count(),
            1
        );
    }

    #[test]
    fn construction_and_deserialization_reject_invalid_boundaries() {
        for starts in [
            vec![],
            vec![1],
            vec![0, 0],
            vec![0, 3, 2],
            vec![0, 4],
            vec![0, usize::MAX],
        ] {
            assert_eq!(
                RegimePartition::new(sequence(), starts.clone()),
                Err(InvalidRegimePartition)
            );
            assert!(serde_json::from_value::<RegimePartition>(
                serde_json::json!({"sequence":sequence(), "starts":starts})
            )
            .is_err());
        }
        assert_eq!(
            RegimePartition::new(ObservationSequence::new(vec![]).unwrap(), vec![0]),
            Err(InvalidRegimePartition)
        );
        assert!(serde_json::from_value::<RegimePartition>(
            serde_json::json!({"sequence":sequence(), "starts":[0], "label":"semantic"})
        )
        .is_err());
        assert!(InvalidRegimePartition.to_string().contains("zero"));
    }
}
