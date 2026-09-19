//! Anonymous partitions of an explicitly ordered observation sequence.

use serde::{Deserialize, Serialize};

use crate::{ObservationSequence, OrderedObservation};

/// A partition at explicitly selected observation boundaries. This records
/// intervals, not a claim that their contents are stationary or semantically
/// distinct. Boundaries can be selected from change-point evidence by a harness
/// that records that evidence and the selection parameters in provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RegimeData", into = "RegimeData")]
pub struct RegimePartition(RegimeData);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegimeData {
    sequence: ObservationSequence,
    starts: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidRegimePartition;

impl std::fmt::Display for InvalidRegimePartition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "regime partition requires a nonempty sequence and increasing in-range starts beginning at zero")
    }
}
impl std::error::Error for InvalidRegimePartition {}

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
        Ok(Self(value))
    }
}
impl From<RegimePartition> for RegimeData {
    fn from(value: RegimePartition) -> Self {
        value.0
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
