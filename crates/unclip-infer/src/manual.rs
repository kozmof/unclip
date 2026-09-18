//! Hand-authored observation inference.

use async_trait::async_trait;
use semver::Version;
use serde::Deserialize;
use unclip_observe::{Observation, PartialRanking};
use unclip_plugin::{InferCtx, InferenceOutput, Inferrer, InferrerDescriptor, PluginError, Result};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManualInput {
    observation: Observation,
    #[serde(default)]
    ranking: Option<PartialRanking>,
}

pub struct ManualInferrer {
    descriptor: InferrerDescriptor,
}

impl Default for ManualInferrer {
    fn default() -> Self {
        Self {
            descriptor: InferrerDescriptor {
                id: unclip_epistemic::PluginId::new("infer.manual"),
                version: Version::new(1, 0, 0),
                params_schema: "{\"type\":\"object\",\"additionalProperties\":false}",
            },
        }
    }
}

#[async_trait]
impl Inferrer for ManualInferrer {
    fn descriptor(&self) -> &InferrerDescriptor {
        &self.descriptor
    }

    async fn infer(
        &self,
        ctx: &InferCtx<'_>,
        token: unclip_epistemic::InferenceToken,
    ) -> Result<unclip_epistemic::Inferred<InferenceOutput>> {
        let input: ManualInput = serde_json::from_value(
            ctx.io.request(&ctx.source, ctx.params).await?,
        )
        .map_err(|error| PluginError::Message(format!("invalid manual observation: {error}")))?;
        if let Some(ranking) = &input.ranking {
            if ranking.observation != input.observation.id {
                return Err(PluginError::Message(
                    "manual ranking observation id does not match the observation".into(),
                ));
            }
        }
        Ok(token.emit(InferenceOutput::Bundle {
            observations: vec![input.observation],
            alignments: Vec::new(),
            rankings: input.ranking.into_iter().collect(),
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
    use unclip_observe::ObservationId;
    use unclip_plugin::{InferenceIo, Inferrer};

    use super::*;

    struct FixtureIo;

    #[async_trait]
    impl InferenceIo for FixtureIo {
        async fn request(
            &self,
            source: &SourceRef,
            params: &serde_json::Value,
        ) -> Result<serde_json::Value> {
            assert_eq!(source.0, "observations/manual.yaml");
            assert_eq!(params, &serde_json::json!({"strict": true}));
            Ok(
                serde_json::from_str(include_str!("../tests/fixtures/manual.json"))
                    .expect("valid manual.json fixture"),
            )
        }
    }

    #[tokio::test]
    async fn emits_observation_and_optional_ranking_with_provenance() {
        let domain = DomainSnapshot {
            id: DomainId::new("domain"),
            version: DomainVersion::new("1"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let params = serde_json::json!({"strict": true});
        let source = SourceRef::new("observations/manual.yaml");
        let ctx = InferCtx {
            source: source.clone(),
            domain: &domain,
            params: &params,
            io: &FixtureIo,
        };
        let metadata = EmitMetadata {
            id: DerivedId::new("manual-derived"),
            producer: PluginId::new("infer.manual"),
            algorithm: "manual".into(),
            version: Version::new(1, 0, 0),
            params: params.clone(),
            params_hash: hash_params(&params),
            source: Some(source.clone()),
            timestamp: Timestamp::new("2026-09-18T00:00:00Z"),
            domain_version: Some(domain.version.clone()),
            frame_version: None,
            model: None,
        };
        let token = unclip_epistemic::InferenceToken::from_harness(
            metadata,
            DependencyCollector::default(),
        );

        let output = ManualInferrer::default().infer(&ctx, token).await.unwrap();

        match output.value() {
            InferenceOutput::Bundle {
                observations,
                alignments,
                rankings,
            } => {
                assert_eq!(observations[0].id, ObservationId::new("manual"));
                assert_eq!(observations[0].units[0].uncertainty, Some(0.1));
                assert!(alignments.is_empty());
                assert_eq!(rankings.len(), 1);
                assert_eq!(rankings[0].tiers[0].units.len(), 2);
            }
            other => panic!("unexpected output: {other:?}"),
        }
        assert_eq!(output.provenance().source.as_ref(), Some(&source));
        assert_eq!(output.provenance().params, params);
        assert_eq!(
            output.provenance().domain_version.as_ref(),
            Some(&domain.version)
        );
    }
}
