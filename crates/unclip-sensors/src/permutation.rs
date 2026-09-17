use std::collections::{BTreeMap, BTreeSet};

use semver::Version;
use unclip_domain::UnitId;
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_measure::{
    Measurement, MeasurementContext, MeasurementKind, MeasurementValue, RankedState, Reading,
};
use unclip_observe::{ObservationId, ObservedUnitId};
use unclip_plugin::{
    Applicability, Capability, EvidenceRequirement, MeasureCtx, Result, Sensor, SensorDescriptor,
};

const APPLICABILITY: &[Capability] = &[Capability::Alignment, Capability::RankingValue];
const EVIDENCE: &[EvidenceRequirement] = &[EvidenceRequirement::MinSamples(1)];
const PRODUCES: &[MeasurementKind] = &[MeasurementKind::Ranking];

pub struct PermutationSensor {
    descriptor: SensorDescriptor,
}

impl Default for PermutationSensor {
    fn default() -> Self {
        Self {
            descriptor: SensorDescriptor {
                id: PluginId::new("sensor.permutation"),
                version: Version::new(0, 1, 0),
                applicability: APPLICABILITY,
                evidence: EVIDENCE,
                produces: PRODUCES,
                params_schema: r#"{"type":"object","additionalProperties":false}"#,
            },
        }
    }
}

impl Sensor for PermutationSensor {
    fn descriptor(&self) -> &SensorDescriptor {
        &self.descriptor
    }

    fn applies_to(&self, ctx: &MeasureCtx<'_>) -> Applicability {
        if ctx.rankings().is_empty() {
            return Applicability::NotApplicable {
                reason: "permutation requires a partial ranking".into(),
            };
        }
        if ctx.alignments().is_empty() {
            return Applicability::NotApplicable {
                reason: "permutation requires alignments".into(),
            };
        }
        if ctx.frame().axes.is_empty() {
            return Applicability::NotApplicable {
                reason: "permutation requires frame axes".into(),
            };
        }
        Applicability::Applicable
    }

    fn measure(
        &self,
        ctx: &MeasureCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<Measurement>>> {
        let frame_units = ctx
            .frame()
            .axes
            .iter()
            .map(|axis| axis.unit.clone())
            .collect::<BTreeSet<_>>();
        let alignments = alignment_index(ctx, &frame_units);

        Ok(ctx
            .rankings()
            .iter()
            .map(|tracked| {
                let ranking = ctx.read(tracked);
                let measurement = match alignments.get(&ranking.observation) {
                    Some(alignment) => {
                        let state = ranked_state(ranking, alignment, &frame_units);
                        let sample_count =
                            state.tiers.iter().map(Vec::len).sum::<usize>() + state.unknown.len();
                        Measurement {
                            sensor: self.descriptor.id.clone(),
                            sensor_version: self.descriptor.version.clone(),
                            reading: Reading::Value {
                                value: MeasurementValue::Ranking(state),
                            },
                            confidence: None,
                            sample_count: Some(sample_count),
                            context: MeasurementContext {
                                values: BTreeMap::from([(
                                    "observation".into(),
                                    serde_json::json!(ranking.observation.0),
                                )]),
                            },
                        }
                    }
                    None => Measurement {
                        sensor: self.descriptor.id.clone(),
                        sensor_version: self.descriptor.version.clone(),
                        reading: Reading::InsufficientEvidence { have: 0, need: 1 },
                        confidence: None,
                        sample_count: Some(0),
                        context: MeasurementContext {
                            values: BTreeMap::from([(
                                "observation".into(),
                                serde_json::json!(ranking.observation.0),
                            )]),
                        },
                    },
                };
                token.emit(measurement)
            })
            .collect())
    }
}

type AlignmentIndex = BTreeMap<ObservationId, BTreeMap<ObservedUnitId, BTreeSet<UnitId>>>;

fn alignment_index(ctx: &MeasureCtx<'_>, frame_units: &BTreeSet<UnitId>) -> AlignmentIndex {
    let mut index: AlignmentIndex = BTreeMap::new();
    for tracked in ctx.alignments() {
        let alignment = ctx.read(tracked);
        let units = index.entry(alignment.observation.clone()).or_default();
        for candidate in &alignment.candidates {
            if frame_units.contains(&candidate.domain) {
                units
                    .entry(candidate.observed.clone())
                    .or_default()
                    .insert(candidate.domain.clone());
            }
        }
    }
    index
}

fn ranked_state(
    ranking: &unclip_observe::PartialRanking,
    alignment: &BTreeMap<ObservedUnitId, BTreeSet<UnitId>>,
    frame_units: &BTreeSet<UnitId>,
) -> RankedState {
    let mut state = RankedState {
        tiers: Vec::new(),
        unknown: Vec::new(),
        unresolved: Vec::new(),
    };
    let mut represented = BTreeSet::new();

    for tier in &ranking.tiers {
        let mut resolved_tier = Vec::new();
        for observed in &tier.units {
            match unique_alignment(alignment.get(observed)) {
                Some(unit) if represented.insert(unit.clone()) => resolved_tier.push(unit.clone()),
                _ => state.unresolved.push(observed.clone()),
            }
        }
        if !resolved_tier.is_empty() {
            state.tiers.push(resolved_tier);
        }
    }

    for observed in &ranking.unknown {
        match unique_alignment(alignment.get(observed)) {
            Some(unit) if represented.insert(unit.clone()) => state.unknown.push(unit.clone()),
            _ => state.unresolved.push(observed.clone()),
        }
    }

    state.unknown.extend(
        frame_units
            .iter()
            .filter(|unit| !represented.contains(*unit))
            .cloned(),
    );
    state
}

fn unique_alignment(candidates: Option<&BTreeSet<UnitId>>) -> Option<&UnitId> {
    let candidates = candidates?;
    (candidates.len() == 1)
        .then(|| candidates.iter().next())
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use unclip_domain::{DomainId, DomainSnapshot, FrameAxis, FrameId, MeasurementFrame};
    use unclip_epistemic::{
        hash_params, DependencyCollector, DerivedId, DomainVersion, EmitMetadata, FrameVersion,
        InferenceToken, SourceRef, Timestamp, Tracked,
    };
    use unclip_observe::{
        Alignment, AlignmentCandidate, Observation, ObservationId, ObservedUnit, PartialRanking,
        RankTier,
    };
    use unclip_plugin::conformance;

    use super::*;

    fn metadata(id: &str, producer: &str) -> EmitMetadata {
        let params = serde_json::json!({});
        EmitMetadata {
            id: DerivedId::new(id),
            producer: PluginId::new(producer),
            algorithm: producer.into(),
            version: Version::new(0, 1, 0),
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
            domain_version: None,
            frame_version: None,
            model: None,
        }
    }

    #[test]
    fn preserves_partial_state_and_unknown_frame_axes() {
        let sensory = UnitId::new("sensory");
        let social = UnitId::new("social");
        let production = UnitId::new("production");
        let domain = DomainSnapshot {
            id: DomainId::new("coffee"),
            version: DomainVersion::new("1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("coffee.general"),
            version: FrameVersion::new("1"),
            axes: vec![
                FrameAxis {
                    unit: production.clone(),
                    label: None,
                },
                FrameAxis {
                    unit: sensory.clone(),
                    label: None,
                },
                FrameAxis {
                    unit: social.clone(),
                    label: None,
                },
            ],
        };
        let observed_sensory = ObservedUnitId::new("appearance");
        let observed_social = ObservedUnitId::new("sharing");
        let observation_id = ObservationId::new("x-1");
        let observation = Observation {
            id: observation_id.clone(),
            source: SourceRef::new("fixture"),
            observed_at: None,
            units: vec![
                ObservedUnit {
                    id: observed_sensory.clone(),
                    label: "appearance".into(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                },
                ObservedUnit {
                    id: observed_social.clone(),
                    label: "sharing".into(),
                    salience: None,
                    uncertainty: None,
                    context: BTreeMap::new(),
                },
            ],
            relations: Vec::new(),
            context: BTreeMap::new(),
        };
        let alignment = Alignment {
            observation: observation_id.clone(),
            candidates: vec![
                AlignmentCandidate {
                    observed: observed_sensory.clone(),
                    domain: sensory.clone(),
                    confidence: 0.9,
                    evidence: Vec::new(),
                },
                AlignmentCandidate {
                    observed: observed_social.clone(),
                    domain: social.clone(),
                    confidence: 0.8,
                    evidence: Vec::new(),
                },
            ],
        };
        let ranking = PartialRanking {
            observation: observation_id,
            tiers: vec![
                RankTier {
                    units: vec![observed_sensory],
                },
                RankTier {
                    units: vec![observed_social],
                },
            ],
            unknown: Vec::new(),
        };
        let observation = InferenceToken::from_harness(
            metadata("observation", "infer.fixture"),
            Default::default(),
        )
        .emit(observation);
        let alignment = InferenceToken::from_harness(
            metadata("alignment", "infer.fixture"),
            Default::default(),
        )
        .emit(alignment);
        let ranking =
            InferenceToken::from_harness(metadata("ranking", "infer.fixture"), Default::default())
                .emit(ranking);
        let observations = vec![Tracked::from(&observation)];
        let alignments = vec![Tracked::from(&alignment)];
        let rankings = vec![Tracked::from(&ranking)];
        let params = serde_json::json!({});
        let ctx = MeasureCtx::new(
            &domain,
            &frame,
            &observations,
            &alignments,
            &rankings,
            &params,
            DependencyCollector::default(),
        );
        let sensor = PermutationSensor::default();

        conformance::assert_sensor(&sensor, |plugin| {
            plugin.measure(
                &ctx,
                ctx.calculation_token(metadata("permutation", "sensor.permutation")),
            )
        });
        let values = sensor
            .measure(
                &ctx,
                ctx.calculation_token(metadata("permutation", "sensor.permutation")),
            )
            .unwrap();
        assert!(matches!(
            &values[0].value().reading,
            Reading::Value {
                value: MeasurementValue::Ranking(state)
            } if state.tiers == vec![vec![sensory], vec![social]]
                && state.unknown == vec![production]
                && state.unresolved.is_empty()
        ));
        assert_eq!(
            values[0].provenance().inputs,
            vec![DerivedId::new("alignment"), DerivedId::new("ranking")]
        );
    }
}
