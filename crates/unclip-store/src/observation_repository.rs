//! Persistence for observations, alignments, and partial rankings.

use std::collections::BTreeMap;

use anyhow::Context;
use async_trait::async_trait;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, TransactionTrait,
};
use unclip_domain::{DomainId, UnitId};
use unclip_entity::{
    alignment_candidates, alignments, domain_versions, observations, observed_relations,
    observed_units, ranking_entries, rankings,
};
use unclip_epistemic::{DerivedId, DomainVersion};
use unclip_observe::{
    Alignment, AlignmentCandidate, Observation, ObservationId, ObservedRelation,
    ObservedRelationId, ObservedUnit, ObservedUnitId, PartialRanking, RankTier,
};

use crate::{StoreError, StoreResult};

#[async_trait]
pub trait ObservationRepository: Sync {
    async fn insert_observation(
        &self,
        observation: Observation,
        provenance: &DerivedId,
    ) -> StoreResult<()>;
    async fn get_observation(&self, id: &ObservationId) -> StoreResult<Option<Observation>>;
    async fn insert_alignment(
        &self,
        id: &str,
        alignment: Alignment,
        domain_id: &DomainId,
        domain_version: &DomainVersion,
        provenance: &DerivedId,
    ) -> StoreResult<()>;
    async fn get_alignment(&self, id: &str) -> StoreResult<Option<Alignment>>;
    async fn insert_ranking(
        &self,
        id: &str,
        ranking: PartialRanking,
        provenance: &DerivedId,
    ) -> StoreResult<()>;
    async fn get_ranking(&self, id: &str) -> StoreResult<Option<PartialRanking>>;
}

pub struct SeaOrmObservationRepository {
    db: DatabaseConnection,
}

impl SeaOrmObservationRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidRequest {
        message: message.into(),
    }
}

fn validate_probability(name: &str, value: Option<f64>) -> StoreResult<()> {
    if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err(invalid(format!(
            "{name} must be finite and between zero and one"
        )));
    }
    Ok(())
}

async fn insert_observation_rows(
    txn: &DatabaseTransaction,
    observation: Observation,
    provenance: &DerivedId,
) -> StoreResult<()> {
    if observations::Entity::find_by_id(&observation.id.0)
        .one(txn)
        .await?
        .is_some()
    {
        return Err(StoreError::AlreadyExists {
            path: observation.id.0,
        });
    }
    observations::Entity::insert(observations::ActiveModel {
        id: Set(observation.id.0.clone()),
        provenance_id: Set(provenance.0.clone()),
        source: Set(observation.source.0),
        observed_at: Set(observation.observed_at),
        context_json: Set(serde_json::to_string(&observation.context).map_err(anyhow::Error::from)?),
    })
    .exec(txn)
    .await?;

    for unit in observation.units {
        validate_probability("unit uncertainty", unit.uncertainty)?;
        if unit.salience.is_some_and(|value| !value.is_finite()) {
            return Err(invalid("unit salience must be finite"));
        }
        observed_units::Entity::insert(observed_units::ActiveModel {
            observation_id: Set(observation.id.0.clone()),
            id: Set(unit.id.0),
            label: Set(unit.label),
            salience: Set(unit.salience),
            uncertainty: Set(unit.uncertainty),
            context_json: Set(serde_json::to_string(&unit.context).map_err(anyhow::Error::from)?),
            provenance_id: Set(provenance.0.clone()),
        })
        .exec(txn)
        .await?;
    }
    for relation in observation.relations {
        validate_probability("relation uncertainty", relation.uncertainty)?;
        observed_relations::Entity::insert(observed_relations::ActiveModel {
            observation_id: Set(observation.id.0.clone()),
            id: Set(relation.id.0),
            source_unit_id: Set(relation.source.0),
            target_unit_id: Set(relation.target.0),
            kind: Set(relation.kind),
            uncertainty: Set(relation.uncertainty),
            provenance_id: Set(provenance.0.clone()),
        })
        .exec(txn)
        .await?;
    }
    Ok(())
}

#[async_trait]
impl ObservationRepository for SeaOrmObservationRepository {
    async fn insert_observation(
        &self,
        observation: Observation,
        provenance: &DerivedId,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        insert_observation_rows(&txn, observation, provenance).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn get_observation(&self, id: &ObservationId) -> StoreResult<Option<Observation>> {
        let Some(row) = observations::Entity::find_by_id(&id.0)
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        let unit_rows = observed_units::Entity::find()
            .filter(observed_units::Column::ObservationId.eq(&id.0))
            .order_by_asc(observed_units::Column::Id)
            .all(&self.db)
            .await?;
        let relation_rows = observed_relations::Entity::find()
            .filter(observed_relations::Column::ObservationId.eq(&id.0))
            .order_by_asc(observed_relations::Column::Id)
            .all(&self.db)
            .await?;

        let units = unit_rows
            .into_iter()
            .map(|unit| {
                Ok(ObservedUnit {
                    id: ObservedUnitId::new(unit.id),
                    label: unit.label,
                    salience: unit.salience,
                    uncertainty: unit.uncertainty,
                    context: serde_json::from_str(&unit.context_json)
                        .context("invalid stored observed-unit context")?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let relations = relation_rows
            .into_iter()
            .map(|relation| ObservedRelation {
                id: ObservedRelationId::new(relation.id),
                source: ObservedUnitId::new(relation.source_unit_id),
                target: ObservedUnitId::new(relation.target_unit_id),
                kind: relation.kind,
                uncertainty: relation.uncertainty,
            })
            .collect();

        Ok(Some(Observation {
            id: ObservationId::new(row.id),
            source: unclip_epistemic::SourceRef::new(row.source),
            observed_at: row.observed_at,
            units,
            relations,
            context: serde_json::from_str(&row.context_json)
                .context("invalid stored observation context")?,
        }))
    }

    async fn insert_alignment(
        &self,
        id: &str,
        alignment: Alignment,
        domain_id: &DomainId,
        domain_version: &DomainVersion,
        provenance: &DerivedId,
    ) -> StoreResult<()> {
        if id.is_empty() {
            return Err(invalid("alignment id must not be empty"));
        }
        let txn = self.db.begin().await?;
        let domain_version_id = domain_versions::Entity::find()
            .filter(domain_versions::Column::DomainId.eq(&domain_id.0))
            .filter(domain_versions::Column::Version.eq(&domain_version.0))
            .one(&txn)
            .await?
            .ok_or_else(|| StoreError::NotFound {
                path: format!("domain {} version {}", domain_id.0, domain_version.0),
            })?
            .id;
        if alignments::Entity::find_by_id(id)
            .one(&txn)
            .await?
            .is_some()
        {
            return Err(StoreError::AlreadyExists { path: id.into() });
        }
        alignments::Entity::insert(alignments::ActiveModel {
            id: Set(id.into()),
            observation_id: Set(alignment.observation.0.clone()),
            provenance_id: Set(provenance.0.clone()),
        })
        .exec(&txn)
        .await?;
        for (position, candidate) in alignment.candidates.into_iter().enumerate() {
            validate_probability("alignment confidence", Some(candidate.confidence))?;
            let position = i32::try_from(position).context("too many alignment candidates")?;
            alignment_candidates::Entity::insert(alignment_candidates::ActiveModel {
                alignment_id: Set(id.into()),
                observation_id: Set(alignment.observation.0.clone()),
                observed_unit_id: Set(candidate.observed.0),
                domain_version_id: Set(domain_version_id.clone()),
                domain_unit_id: Set(candidate.domain.0),
                position: Set(position),
                confidence: Set(candidate.confidence),
                evidence_json: Set(
                    serde_json::to_string(&candidate.evidence).map_err(anyhow::Error::from)?
                ),
                provenance_id: Set(provenance.0.clone()),
            })
            .exec(&txn)
            .await?;
        }
        txn.commit().await?;
        Ok(())
    }

    async fn get_alignment(&self, id: &str) -> StoreResult<Option<Alignment>> {
        let Some(row) = alignments::Entity::find_by_id(id).one(&self.db).await? else {
            return Ok(None);
        };
        let candidates = alignment_candidates::Entity::find()
            .filter(alignment_candidates::Column::AlignmentId.eq(id))
            .order_by_asc(alignment_candidates::Column::Position)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|candidate| {
                Ok(AlignmentCandidate {
                    observed: ObservedUnitId::new(candidate.observed_unit_id),
                    domain: UnitId::new(candidate.domain_unit_id),
                    confidence: candidate.confidence,
                    evidence: serde_json::from_str(&candidate.evidence_json)
                        .context("invalid stored alignment evidence")?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Some(Alignment {
            observation: ObservationId::new(row.observation_id),
            candidates,
        }))
    }

    async fn insert_ranking(
        &self,
        id: &str,
        ranking: PartialRanking,
        provenance: &DerivedId,
    ) -> StoreResult<()> {
        if id.is_empty() {
            return Err(invalid("ranking id must not be empty"));
        }
        let txn = self.db.begin().await?;
        if rankings::Entity::find_by_id(id).one(&txn).await?.is_some() {
            return Err(StoreError::AlreadyExists { path: id.into() });
        }
        rankings::Entity::insert(rankings::ActiveModel {
            id: Set(id.into()),
            observation_id: Set(ranking.observation.0.clone()),
            provenance_id: Set(provenance.0.clone()),
        })
        .exec(&txn)
        .await?;
        for (tier, group) in ranking.tiers.into_iter().enumerate() {
            let tier = i32::try_from(tier).context("too many ranking tiers")?;
            for (position, unit) in group.units.into_iter().enumerate() {
                let position = i32::try_from(position).context("ranking tier is too large")?;
                ranking_entries::Entity::insert(ranking_entries::ActiveModel {
                    ranking_id: Set(id.into()),
                    observation_id: Set(ranking.observation.0.clone()),
                    observed_unit_id: Set(unit.0),
                    state: Set("ranked".into()),
                    tier: Set(tier),
                    position: Set(position),
                    provenance_id: Set(provenance.0.clone()),
                })
                .exec(&txn)
                .await?;
            }
        }
        for (position, unit) in ranking.unknown.into_iter().enumerate() {
            let position = i32::try_from(position).context("ranking unknown tail is too large")?;
            ranking_entries::Entity::insert(ranking_entries::ActiveModel {
                ranking_id: Set(id.into()),
                observation_id: Set(ranking.observation.0.clone()),
                observed_unit_id: Set(unit.0),
                state: Set("unknown".into()),
                tier: Set(-1),
                position: Set(position),
                provenance_id: Set(provenance.0.clone()),
            })
            .exec(&txn)
            .await?;
        }
        txn.commit().await?;
        Ok(())
    }

    async fn get_ranking(&self, id: &str) -> StoreResult<Option<PartialRanking>> {
        let Some(row) = rankings::Entity::find_by_id(id).one(&self.db).await? else {
            return Ok(None);
        };
        let entries = ranking_entries::Entity::find()
            .filter(ranking_entries::Column::RankingId.eq(id))
            .order_by_asc(ranking_entries::Column::Tier)
            .order_by_asc(ranking_entries::Column::Position)
            .all(&self.db)
            .await?;
        let mut tiers: BTreeMap<i32, Vec<ObservedUnitId>> = BTreeMap::new();
        let mut unknown = Vec::new();
        for entry in entries {
            match entry.state.as_str() {
                "ranked" => tiers
                    .entry(entry.tier)
                    .or_default()
                    .push(ObservedUnitId::new(entry.observed_unit_id)),
                "unknown" => unknown.push(ObservedUnitId::new(entry.observed_unit_id)),
                other => return Err(invalid(format!("unknown stored ranking state: {other}"))),
            }
        }
        Ok(Some(PartialRanking {
            observation: ObservationId::new(row.observation_id),
            tiers: tiers
                .into_values()
                .map(|units| RankTier { units })
                .collect(),
            unknown,
        }))
    }
}
