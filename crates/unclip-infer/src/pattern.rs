//! Pattern-dictionary observation inference.

use async_trait::async_trait;
use semver::Version;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use unclip_domain::{RelationId, UnitId};
use unclip_match::{Matcher, PatternEntry, PatternTarget};
use unclip_observe::{
    Alignment, AlignmentCandidate, Observation, ObservationId, ObservedRelation,
    ObservedRelationId, ObservedUnit, ObservedUnitId,
};
use unclip_plugin::{InferCtx, InferenceOutput, Inferrer, InferrerDescriptor, PluginError, Result};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatternInput {
    text: String,
    patterns: Vec<PatternEntry>,
    #[serde(default = "default_id")]
    observation_id: String,
    #[serde(default)]
    observed_at: Option<String>,
}
fn default_id() -> String {
    "pattern-observation".into()
}

pub struct PatternInferrer {
    descriptor: InferrerDescriptor,
}
impl Default for PatternInferrer {
    fn default() -> Self {
        Self {
            descriptor: InferrerDescriptor {
                id: unclip_epistemic::PluginId::new("infer.pattern"),
                version: Version::new(1, 0, 0),
                params_schema: "{\"type\":\"object\",\"properties\":{\"min_confidence\":{\"type\":\"number\",\"minimum\":0.0,\"maximum\":1.0}},\"additionalProperties\":false}",
            },
        }
    }
}
fn value(target: &PatternTarget) -> &str {
    match target {
        PatternTarget::O2m { value, .. } | PatternTarget::O2o { value, .. } => value,
        PatternTarget::Branch { path } | PatternTarget::CollapsePattern { path } => path,
    }
}

#[async_trait]
impl Inferrer for PatternInferrer {
    fn descriptor(&self) -> &InferrerDescriptor {
        &self.descriptor
    }
    async fn infer(
        &self,
        ctx: &InferCtx<'_>,
        token: unclip_epistemic::InferenceToken,
    ) -> Result<unclip_epistemic::Inferred<InferenceOutput>> {
        let input: PatternInput =
            serde_json::from_value(ctx.io.request(&ctx.source, ctx.params).await?)
                .map_err(|e| PluginError::Message(format!("invalid pattern input: {e}")))?;
        let confidence = ctx
            .params
            .get("min_confidence")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0);
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(PluginError::Message(
                "pattern min_confidence must be between zero and one".into(),
            ));
        }
        let matcher =
            Matcher::build(input.patterns).map_err(|e| PluginError::Message(e.to_string()))?;
        let mut spans = BTreeMap::<(usize, usize), Vec<_>>::new();
        for hit in matcher.scan(&input.text) {
            spans.entry((hit.start, hit.end)).or_default().push(hit);
        }
        let observation_id = ObservationId::new(input.observation_id);
        let mut units = Vec::new();
        let mut candidates = Vec::new();
        let mut mapped = BTreeMap::<UnitId, ObservedUnitId>::new();
        let mut relation_hits = BTreeSet::<RelationId>::new();
        for ((start, end), hits) in spans {
            let domain_units: Vec<_> = hits
                .iter()
                .filter_map(|hit| {
                    let id = UnitId::new(value(&hit.target));
                    ctx.domain
                        .units
                        .contains_key(&id)
                        .then_some((id, hit.pattern.clone()))
                })
                .collect();
            for hit in &hits {
                let id = RelationId::new(value(&hit.target));
                if ctx.domain.relations.contains_key(&id) {
                    relation_hits.insert(id);
                }
            }
            if domain_units.is_empty() {
                continue;
            }
            let observed = ObservedUnitId::new(format!("hit-{start}-{end}"));
            units.push(ObservedUnit {
                id: observed.clone(),
                label: input.text[start..end].into(),
                salience: Some(1.0),
                uncertainty: Some(1.0 - confidence),
                context: BTreeMap::new(),
            });
            for (domain, pattern) in domain_units {
                mapped
                    .entry(domain.clone())
                    .or_insert_with(|| observed.clone());
                candidates.push(AlignmentCandidate {
                    observed: observed.clone(),
                    domain,
                    confidence,
                    evidence: vec![pattern],
                });
            }
        }
        let relations = relation_hits
            .into_iter()
            .filter_map(|id| {
                let relation = &ctx.domain.relations[&id];
                Some(ObservedRelation {
                    id: ObservedRelationId::new(format!("relation-{}", id.0)),
                    source: mapped.get(&relation.source)?.clone(),
                    target: mapped.get(&relation.target)?.clone(),
                    kind: relation.kind.clone(),
                    uncertainty: Some(1.0 - confidence),
                })
            })
            .collect();
        let observation = Observation {
            id: observation_id.clone(),
            source: ctx.source.clone(),
            observed_at: input.observed_at,
            units,
            relations,
            context: BTreeMap::new(),
        };
        Ok(token.emit(InferenceOutput::Bundle {
            observations: vec![observation],
            alignments: vec![Alignment {
                observation: observation_id,
                candidates,
            }],
            rankings: Vec::new(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use unclip_domain::{DomainId, DomainSnapshot, Relation, RelationId, Unit, UnitId, UnitKind};
    use unclip_epistemic::{
        hash_params, DependencyCollector, DerivedId, DomainVersion, EmitMetadata, PluginId,
        SourceRef, Timestamp,
    };
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
            assert_eq!(source.0, "notes/pattern.txt");
            assert_eq!(params, &serde_json::json!({"min_confidence": 0.7}));
            Ok(serde_json::json!({
                "text": "alpha links beta",
                "observation_id": "pattern-result",
                "observed_at": "2026-09-18T00:00:00Z",
                "patterns": [
                    {
                        "pattern": "alpha",
                        "target": {"kind": "o2o", "name": "unit", "value": "u1"}
                    },
                    {
                        "pattern": "alpha",
                        "target": {"kind": "o2o", "name": "unit", "value": "u3"}
                    },
                    {
                        "pattern": "beta",
                        "target": {"kind": "o2o", "name": "unit", "value": "u2"}
                    },
                    {
                        "pattern": "links",
                        "target": {"kind": "o2o", "name": "relation", "value": "r1"}
                    }
                ]
            }))
        }
    }

    fn unit(id: &str) -> Unit {
        Unit {
            id: UnitId::new(id),
            kind: UnitKind::AtomicMeaning,
            label: None,
            properties: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn extracts_units_relations_and_ambiguous_alignments() {
        let u1 = UnitId::new("u1");
        let u2 = UnitId::new("u2");
        let domain = DomainSnapshot {
            id: DomainId::new("domain"),
            version: DomainVersion::new("1"),
            units: [
                (u1.clone(), unit("u1")),
                (u2.clone(), unit("u2")),
                (UnitId::new("u3"), unit("u3")),
            ]
            .into_iter()
            .collect(),
            relations: [(
                RelationId::new("r1"),
                Relation {
                    id: RelationId::new("r1"),
                    source: u1,
                    target: u2,
                    kind: "links".into(),
                    properties: BTreeMap::new(),
                },
            )]
            .into_iter()
            .collect(),
        };
        let params = serde_json::json!({"min_confidence": 0.7});
        let source = SourceRef::new("notes/pattern.txt");
        let ctx = InferCtx {
            source: source.clone(),
            domain: &domain,
            params: &params,
            io: &FixtureIo,
        };
        let token = unclip_epistemic::InferenceToken::from_harness(
            EmitMetadata {
                id: DerivedId::new("pattern-derived"),
                producer: PluginId::new("infer.pattern"),
                algorithm: "pattern".into(),
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

        let output = PatternInferrer::default().infer(&ctx, token).await.unwrap();

        match output.value() {
            InferenceOutput::Bundle {
                observations,
                alignments,
                rankings,
            } => {
                assert!(rankings.is_empty());
                assert_eq!(observations.len(), 1);
                assert_eq!(observations[0].id, ObservationId::new("pattern-result"));
                assert_eq!(observations[0].units.len(), 2);
                assert_eq!(observations[0].relations.len(), 1);
                assert_eq!(observations[0].relations[0].kind, "links");
                assert_eq!(alignments.len(), 1);
                assert_eq!(alignments[0].candidates.len(), 3);

                let alpha = &observations[0].units[0].id;
                let alpha_candidates: Vec<_> = alignments[0]
                    .candidates
                    .iter()
                    .filter(|candidate| &candidate.observed == alpha)
                    .collect();
                assert_eq!(alpha_candidates.len(), 2);
                assert_eq!(alpha_candidates[0].confidence, 0.7);
                assert_eq!(alpha_candidates[1].confidence, 0.7);
                assert_ne!(alpha_candidates[0].domain, alpha_candidates[1].domain);
            }
            other => panic!("unexpected output: {other:?}"),
        }
        assert_eq!(output.provenance().source.as_ref(), Some(&source));
        assert_eq!(output.provenance().params, params);
    }
}
