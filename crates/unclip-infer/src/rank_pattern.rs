//! Pattern-evidence salience and partial-ranking inference.

use std::cmp::Ordering;

use async_trait::async_trait;
use semver::Version;
use serde::Deserialize;
use unclip_match::{Matcher, PatternEntry, PatternTarget};
use unclip_observe::{Observation, PartialRanking, RankTier};
use unclip_plugin::{InferCtx, InferenceOutput, Inferrer, InferrerDescriptor, PluginError, Result};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RankPatternEvidence {
    pattern: String,
    salience: f64,
    uncertainty: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RankPatternInput {
    observation: Observation,
    evidence: Vec<RankPatternEvidence>,
}

pub struct RankPatternInferrer {
    descriptor: InferrerDescriptor,
}

impl Default for RankPatternInferrer {
    fn default() -> Self {
        Self {
            descriptor: InferrerDescriptor {
                id: unclip_epistemic::PluginId::new("infer.rank-pattern"),
                version: Version::new(1, 0, 0),
                params_schema: "{\"type\":\"object\",\"properties\":{\"ties\":{\"const\":\"preserve\"},\"unknown_tail\":{\"const\":\"preserve\"}},\"additionalProperties\":false}",
            },
        }
    }
}

fn validate_params(params: &serde_json::Value) -> Result<()> {
    for (name, expected) in [("ties", "preserve"), ("unknown_tail", "preserve")] {
        if let Some(value) = params.get(name) {
            if value.as_str() != Some(expected) {
                return Err(PluginError::Message(format!(
                    "rank-pattern {name} must be \"{expected}\""
                )));
            }
        }
    }
    Ok(())
}

fn rank_observation(
    mut observation: Observation,
    evidence: Vec<RankPatternEvidence>,
) -> Result<(Observation, PartialRanking)> {
    let mut entries = Vec::with_capacity(evidence.len());
    for (index, item) in evidence.iter().enumerate() {
        if item.pattern.trim().is_empty() {
            return Err(PluginError::Message(
                "rank-pattern evidence must have a non-empty pattern".into(),
            ));
        }
        if !item.salience.is_finite() {
            return Err(PluginError::Message(
                "rank-pattern salience must be finite".into(),
            ));
        }
        if !item.uncertainty.is_finite() || !(0.0..=1.0).contains(&item.uncertainty) {
            return Err(PluginError::Message(
                "rank-pattern uncertainty must be between zero and one".into(),
            ));
        }
        entries.push(PatternEntry::new(
            &item.pattern,
            PatternTarget::O2o {
                name: "rank-evidence".into(),
                value: index.to_string(),
            },
        ));
    }

    let matcher =
        Matcher::build(entries).map_err(|error| PluginError::Message(error.to_string()))?;
    let mut known = Vec::new();
    let mut unknown = Vec::new();

    for unit in &mut observation.units {
        let strongest = matcher
            .scan(&unit.label)
            .into_iter()
            .filter_map(|hit| {
                let index = match &hit.target {
                    PatternTarget::O2o { value, .. } => value.parse::<usize>().ok()?,
                    _ => return None,
                };
                evidence.get(index)
            })
            .fold(None::<(f64, f64)>, |best, item| match best {
                None => Some((item.salience, item.uncertainty)),
                Some((salience, uncertainty)) => match item.salience.total_cmp(&salience) {
                    Ordering::Greater => Some((item.salience, item.uncertainty)),
                    Ordering::Equal => Some((salience, uncertainty.max(item.uncertainty))),
                    Ordering::Less => Some((salience, uncertainty)),
                },
            });

        if let Some((salience, uncertainty)) = strongest {
            unit.salience = Some(salience);
            unit.uncertainty = Some(uncertainty);
            known.push((unit.id.clone(), salience));
        } else {
            unit.salience = None;
            unit.uncertainty = None;
            unknown.push(unit.id.clone());
        }
    }

    known.sort_by(|left, right| right.1.total_cmp(&left.1));
    let mut tiers: Vec<RankTier> = Vec::new();
    let mut tier_salience: Vec<f64> = Vec::new();
    for (unit, salience) in known {
        if tier_salience
            .last()
            .is_some_and(|previous| previous.total_cmp(&salience) == Ordering::Equal)
        {
            tiers
                .last_mut()
                .expect("tier salience has a matching tier")
                .units
                .push(unit);
        } else {
            tier_salience.push(salience);
            tiers.push(RankTier { units: vec![unit] });
        }
    }

    let ranking = PartialRanking {
        observation: observation.id.clone(),
        tiers,
        unknown,
    };
    Ok((observation, ranking))
}

#[async_trait]
impl Inferrer for RankPatternInferrer {
    fn descriptor(&self) -> &InferrerDescriptor {
        &self.descriptor
    }

    async fn infer(
        &self,
        ctx: &InferCtx<'_>,
        token: unclip_epistemic::InferenceToken,
    ) -> Result<unclip_epistemic::Inferred<InferenceOutput>> {
        validate_params(ctx.params)?;
        let input: RankPatternInput = serde_json::from_value(
            ctx.io.request(&ctx.source, ctx.params).await?,
        )
        .map_err(|error| PluginError::Message(format!("invalid rank-pattern input: {error}")))?;
        let (observation, ranking) = rank_observation(input.observation, input.evidence)?;
        Ok(token.emit(InferenceOutput::Bundle {
            observations: vec![observation],
            alignments: Vec::new(),
            rankings: vec![ranking],
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use unclip_domain::{DomainId, DomainSnapshot};
    use unclip_epistemic::{
        hash_params, DependencyCollector, DerivedId, DomainVersion, EmitMetadata, PluginId,
        SourceRef, Timestamp,
    };
    use unclip_observe::{ObservationId, ObservedUnitId};
    use unclip_plugin::InferenceIo;

    use super::*;

    struct FixtureIo;

    #[async_trait]
    impl InferenceIo for FixtureIo {
        async fn request(
            &self,
            source: &SourceRef,
            params: &serde_json::Value,
        ) -> Result<serde_json::Value> {
            assert_eq!(source.0, "notes/ranking.yaml");
            assert_eq!(
                params,
                &serde_json::json!({"ties": "preserve", "unknown_tail": "preserve"})
            );
            Ok(
                serde_json::from_str(include_str!("../tests/fixtures/rank-pattern.json"))
                    .expect("valid rank-pattern.json fixture"),
            )
        }
    }

    #[tokio::test]
    async fn preserves_uncertainty_ties_unknown_tail_and_provenance() {
        let domain = DomainSnapshot {
            id: DomainId::new("domain"),
            version: DomainVersion::new("1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let params = serde_json::json!({"ties": "preserve", "unknown_tail": "preserve"});
        let source = SourceRef::new("notes/ranking.yaml");
        let ctx = InferCtx {
            source: source.clone(),
            domain: &domain,
            params: &params,
            io: &FixtureIo,
        };
        let token = unclip_epistemic::InferenceToken::from_harness(
            EmitMetadata {
                id: DerivedId::new("ranking-derived"),
                producer: PluginId::new("infer.rank-pattern"),
                algorithm: "rank-pattern".into(),
                version: Version::new(1, 0, 0),
                params: params.clone(),
                params_hash: hash_params(&params),
                source: Some(source.clone()),
                timestamp: Timestamp::new("2026-09-18T00:00:00Z"),
                domain_version: Some(domain.version.clone()),
                frame_version: None,
                model: None,
            },
            DependencyCollector::default(),
        );

        let output = RankPatternInferrer::default()
            .infer(&ctx, token)
            .await
            .unwrap();

        match output.value() {
            InferenceOutput::Bundle {
                observations,
                alignments,
                rankings,
            } => {
                assert!(alignments.is_empty());
                assert_eq!(observations[0].units[0].salience, Some(0.9));
                assert_eq!(observations[0].units[0].uncertainty, Some(0.3));
                assert_eq!(observations[0].units[2].salience, None);
                assert_eq!(rankings[0].observation, ObservationId::new("ranked"));
                assert_eq!(
                    rankings[0].tiers[0].units,
                    vec![ObservedUnitId::new("a"), ObservedUnitId::new("b")]
                );
                assert_eq!(rankings[0].tiers[1].units, vec![ObservedUnitId::new("d")]);
                assert_eq!(rankings[0].unknown, vec![ObservedUnitId::new("c")]);
                assert!(!rankings[0].is_total());
            }
            other => panic!("unexpected output: {other:?}"),
        }
        assert_eq!(output.provenance().source.as_ref(), Some(&source));
        assert_eq!(output.provenance().params, params);
    }
}
