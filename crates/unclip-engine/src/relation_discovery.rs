//! Recurring unexplained directed relations, retained as relation proposals.
use crate::discovery::{minimum_observations, residual_evidence};
use std::collections::{BTreeMap, BTreeSet};
use unclip_domain::{CandidateKind, CandidateProposal};
use unclip_epistemic::{Calculated, CalculationToken, PluginId};
use unclip_plugin::{CandidateCtx, CandidateGenerator, PluginDescriptor, PluginError, Result};

pub struct MissingRelationGenerator {
    descriptor: PluginDescriptor,
}
impl Default for MissingRelationGenerator {
    fn default() -> Self {
        Self {
            descriptor: PluginDescriptor {
                id: PluginId::new("generate.missing-relation"),
                version: semver::Version::new(0, 1, 0),
                params_schema: r#"{"type":"object","additionalProperties":false,"required":["minimum_observations"],"properties":{"minimum_observations":{"type":"integer","minimum":2}}}"#,
            },
        }
    }
}
fn invalid(message: &str) -> PluginError {
    PluginError::Message(message.into())
}
pub(super) struct ResidualRelation<'a> {
    pub observation: unclip_observe::ObservationId,
    pub relation: &'a unclip_observe::ObservedRelation,
    pub labels: (String, String, String),
    pub measurements: BTreeSet<unclip_epistemic::DerivedId>,
}

pub(super) fn residual_relations<'a>(
    ctx: &'a CandidateCtx<'_>,
) -> Result<Vec<ResidualRelation<'a>>> {
    let mut observations = BTreeSet::new();
    let mut relations = BTreeMap::new();
    for tracked in ctx.observations() {
        let observation = ctx.read(tracked);
        if !observations.insert(&observation.id) {
            return Err(invalid("duplicate discovery observation"));
        }
        let mut units = BTreeMap::new();
        for unit in &observation.units {
            if units.insert(&unit.id, &unit.label).is_some() {
                return Err(invalid("duplicate observed unit identity"));
            }
        }
        for relation in &observation.relations {
            let source = units
                .get(&relation.source)
                .ok_or_else(|| invalid("observed relation source is missing"))?;
            let target = units
                .get(&relation.target)
                .ok_or_else(|| invalid("observed relation target is missing"))?;
            let key = (
                source.to_string(),
                relation.kind.clone(),
                target.to_string(),
            );
            let qualified = format!("{}/{}", observation.id.0, relation.id.0);
            if relations
                .insert(qualified, (observation.id.clone(), relation, key))
                .is_some()
            {
                return Err(invalid("ambiguous qualified residual relation identity"));
            }
        }
    }
    let mut selected = Vec::new();
    for (id, measurements) in residual_evidence(ctx, "unexplained_relations")? {
        let (observation, relation, labels) = relations
            .remove(&id)
            .ok_or_else(|| invalid("residual relation is not present in selected observations"))?;
        if labels.0.trim().is_empty() || labels.1.trim().is_empty() || labels.2.trim().is_empty() {
            continue;
        }
        if relation
            .uncertainty
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(invalid(
                "relation uncertainty must be finite and between zero and one",
            ));
        }
        selected.push(ResidualRelation {
            observation,
            relation,
            labels,
            measurements,
        });
    }
    Ok(selected)
}

impl CandidateGenerator for MissingRelationGenerator {
    fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }
    fn generate(
        &self,
        ctx: &CandidateCtx<'_>,
        token: CalculationToken,
    ) -> Result<Vec<Calculated<CandidateProposal>>> {
        let minimum = minimum_observations(ctx)?;
        let residuals = residual_relations(ctx)?;
        let mut groups = BTreeMap::<_, Vec<serde_json::Value>>::new();
        let mut support = BTreeMap::<_, BTreeSet<_>>::new();
        for edge in residuals {
            let ResidualRelation {
                observation,
                relation,
                labels: key,
                measurements,
            } = edge;
            support
                .entry(key.clone())
                .or_default()
                .insert(observation.clone());
            groups.entry(key.clone()).or_default().push(serde_json::json!({"observation":observation,"relation":relation.id,"source":relation.source,"target":relation.target,"uncertainty":relation.uncertainty,"measurements":measurements}));
        }
        let mut proposals = Vec::new();
        for ((source, kind, target), examples) in groups {
            let observations = &support[&(source.clone(), kind.clone(), target.clone())];
            if observations.len() < minimum {
                continue;
            }
            proposals.push(token.emit(CandidateProposal { domain_version_id:ctx.domain_version_id().into(),kind:CandidateKind::Relation,
                value:serde_json::json!({"pattern":{"matching":"exact_directed_observed_relation","source_label":source,"relation_kind":kind,"target_label":target},"observation_count":observations.len(),"observations":observations,"examples":examples}).as_object().expect("object").clone() }));
        }
        Ok(proposals)
    }
}
