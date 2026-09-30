//! Exact recurring directed two-edge paths in unexplained relation evidence.
use crate::discovery::{relation::residual_relations, residual::minimum_observations};
use std::collections::{BTreeMap, BTreeSet};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_plugin::{CandidateCtx, CandidateGenerator, PluginDescriptor, Result};

pub struct RecurringMotifGenerator {
    descriptor: PluginDescriptor,
}
impl Default for RecurringMotifGenerator {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("generate.recurring-motif"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{
                    "type":"object",
                    "additionalProperties":false,
                    "required":["minimum_observations"],
                    "properties":{"minimum_observations":{"type":"integer","minimum":2}}
                }"#,
            },
        }
    }
}
impl CandidateGenerator for RecurringMotifGenerator {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let minimum = minimum_observations(ctx)?;
        let edges = residual_relations(ctx)?;
        let mut outgoing = BTreeMap::<_, Vec<_>>::new();
        for edge in &edges {
            outgoing
                .entry((&edge.observation, &edge.relation.source))
                .or_default()
                .push(edge);
        }
        let mut groups = BTreeMap::<[&str; 5], Vec<serde_json::Value>>::new();
        let mut support = BTreeMap::<[&str; 5], BTreeSet<_>>::new();
        for first in &edges {
            let Some(next) = outgoing.get(&(&first.observation, &first.relation.target)) else {
                continue;
            };
            for second in next {
                // Match shared unit identities, not merely equal labels, and
                // restrict this plugin to paths through three distinct nodes.
                let a = &first.relation.source;
                let b = &first.relation.target;
                let c = &second.relation.target;
                if a == b || b == c || a == c {
                    continue;
                }
                let key = [
                    first.labels.0,
                    first.labels.1,
                    first.labels.2,
                    second.labels.1,
                    second.labels.2,
                ];
                support.entry(key).or_default().insert(first.observation);
                let edge_evidence = |edge: &crate::discovery::relation::ResidualRelation<'_>| serde_json::json!({"relation":edge.relation.id,"uncertainty":edge.relation.uncertainty,"measurements":edge.measurements});
                groups.entry(key).or_default().push(serde_json::json!({"observation":first.observation,"units":[a,b,c],"edges":[edge_evidence(first),edge_evidence(second)]}));
            }
        }
        let mut candidates = Vec::new();
        for (key, examples) in groups {
            let observations = &support[&key];
            if observations.len() < minimum {
                continue;
            }
            let [source, first_kind, middle, second_kind, target] = key;
            candidates.push(token.emit(CandidateProposal {domain_version_id:ctx.domain_version_id().into(),kind:CandidateKind::GraphMotif,
                value:crate::support::json_object(serde_json::json!({"pattern":{"matching":"exact_directed_two_edge_path","nodes":[{"position":0,"observed_label":source},{"position":1,"observed_label":middle},{"position":2,"observed_label":target}],"edges":[{"source":0,"target":1,"kind":first_kind},{"source":1,"target":2,"kind":second_kind}]},"observation_count":observations.len(),"observations":observations,"examples":examples}))}));
        }
        Ok(candidates)
    }
}
