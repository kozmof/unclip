//! Immutable persistence for versioned semantic domains.

use std::collections::BTreeMap;

use async_trait::async_trait;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, TransactionTrait,
};
use unclip_domain::{
    DomainId, DomainSnapshot, FrameAxis, FrameId, MeasurementFrame, PropertyValue, Relation,
    RelationId, Unit, UnitId, UnitKind,
};
use unclip_entity::{
    domain_versions, domains, frame_axes, frame_versions, measurement_frames, relation_properties,
    relations, unit_properties, units,
};
use unclip_epistemic::{DomainVersion, FrameVersion};

use crate::{now, StoreError, StoreResult};

#[async_trait]
pub trait DomainReader: Sync {
    async fn get_domain_version(
        &self,
        domain_id: &DomainId,
        version: &DomainVersion,
    ) -> StoreResult<Option<DomainSnapshot>>;
    async fn get_measurement_frame(
        &self,
        frame_id: &FrameId,
        version: &FrameVersion,
    ) -> StoreResult<Option<MeasurementFrame>>;
}

#[async_trait]
pub trait DomainWriter: Sync {
    async fn insert_domain_version(&self, snapshot: DomainSnapshot) -> StoreResult<()>;
    async fn insert_measurement_frame(
        &self,
        domain_id: &DomainId,
        domain_version: &DomainVersion,
        frame: MeasurementFrame,
    ) -> StoreResult<()>;
}

pub struct SeaOrmDomainRepository {
    db: DatabaseConnection,
}

impl SeaOrmDomainRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn version_key(domain_id: &DomainId, version: &DomainVersion) -> String {
    serde_json::to_string(&(&domain_id.0, &version.0)).expect("serializing two strings cannot fail")
}

fn frame_version_key(frame_id: &FrameId, version: &FrameVersion) -> String {
    serde_json::to_string(&(&frame_id.0, &version.0)).expect("serializing two strings cannot fail")
}

fn unit_kind_name(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::AtomicMeaning => "atomic_meaning",
        UnitKind::CompositeMeaning => "composite_meaning",
        UnitKind::SemanticRole => "semantic_role",
        UnitKind::GraphMotif => "graph_motif",
        UnitKind::Transformation => "transformation",
        UnitKind::DynamicCoupling => "dynamic_coupling",
        UnitKind::LatentAxis => "latent_axis",
        UnitKind::CrossDomainStructure => "cross_domain_structure",
    }
}

fn parse_unit_kind(value: &str) -> StoreResult<UnitKind> {
    match value {
        "atomic_meaning" => Ok(UnitKind::AtomicMeaning),
        "composite_meaning" => Ok(UnitKind::CompositeMeaning),
        "semantic_role" => Ok(UnitKind::SemanticRole),
        "graph_motif" => Ok(UnitKind::GraphMotif),
        "transformation" => Ok(UnitKind::Transformation),
        "dynamic_coupling" => Ok(UnitKind::DynamicCoupling),
        "latent_axis" => Ok(UnitKind::LatentAxis),
        "cross_domain_structure" => Ok(UnitKind::CrossDomainStructure),
        other => Err(StoreError::InvalidRequest {
            message: format!("unknown stored unit kind: {other}"),
        }),
    }
}

struct PropertyColumns {
    value_kind: String,
    boolean_value: Option<bool>,
    integer_value: Option<i64>,
    number_value: Option<f64>,
    text_value: Option<String>,
    structured_json: Option<String>,
}

fn property_columns(value: PropertyValue) -> StoreResult<PropertyColumns> {
    let mut columns = PropertyColumns {
        value_kind: String::new(),
        boolean_value: None,
        integer_value: None,
        number_value: None,
        text_value: None,
        structured_json: None,
    };
    match value {
        PropertyValue::Boolean(value) => {
            columns.value_kind = "boolean".into();
            columns.boolean_value = Some(value);
        }
        PropertyValue::Integer(value) => {
            columns.value_kind = "integer".into();
            columns.integer_value = Some(value);
        }
        PropertyValue::Number(value) => {
            if !value.is_finite() {
                return Err(StoreError::InvalidRequest {
                    message: "numeric domain properties must be finite".into(),
                });
            }
            columns.value_kind = "number".into();
            columns.number_value = Some(value);
        }
        PropertyValue::Text(value) => {
            columns.value_kind = "text".into();
            columns.text_value = Some(value);
        }
        PropertyValue::Structured(value) => {
            columns.value_kind = "structured".into();
            columns.structured_json =
                Some(serde_json::to_string(&value).map_err(anyhow::Error::from)?);
        }
    }
    Ok(columns)
}

fn parse_property(
    kind: &str,
    boolean: Option<bool>,
    integer: Option<i64>,
    number: Option<f64>,
    text: Option<String>,
    structured: Option<String>,
) -> StoreResult<PropertyValue> {
    let malformed = || StoreError::InvalidRequest {
        message: format!("stored {kind} property has invalid value columns"),
    };
    match kind {
        "boolean" => boolean.map(PropertyValue::Boolean).ok_or_else(malformed),
        "integer" => integer.map(PropertyValue::Integer).ok_or_else(malformed),
        "number" => number.map(PropertyValue::Number).ok_or_else(malformed),
        "text" => text.map(PropertyValue::Text).ok_or_else(malformed),
        "structured" => structured.ok_or_else(malformed).and_then(|value| {
            Ok(PropertyValue::Structured(
                serde_json::from_str(&value).map_err(anyhow::Error::from)?,
            ))
        }),
        other => Err(StoreError::InvalidRequest {
            message: format!("unknown stored property kind: {other}"),
        }),
    }
}

async fn insert_snapshot(txn: &DatabaseTransaction, snapshot: DomainSnapshot) -> StoreResult<()> {
    let key = version_key(&snapshot.id, &snapshot.version);
    if domain_versions::Entity::find_by_id(&key)
        .one(txn)
        .await?
        .is_some()
    {
        return Err(StoreError::AlreadyExists { path: key });
    }

    if domains::Entity::find_by_id(&snapshot.id.0)
        .one(txn)
        .await?
        .is_none()
    {
        domains::Entity::insert(domains::ActiveModel {
            id: Set(snapshot.id.0.clone()),
            label: Set(None),
            created_at: Set(now()),
        })
        .exec(txn)
        .await?;
    }

    domain_versions::Entity::insert(domain_versions::ActiveModel {
        id: Set(key.clone()),
        domain_id: Set(snapshot.id.0),
        version: Set(snapshot.version.0),
        predecessor_id: Set(None),
        created_at: Set(now()),
    })
    .exec(txn)
    .await?;

    for (_, unit) in snapshot.units {
        let unit_id = unit.id.0.clone();
        units::Entity::insert(units::ActiveModel {
            domain_version_id: Set(key.clone()),
            id: Set(unit_id.clone()),
            kind: Set(unit_kind_name(unit.kind).into()),
            label: Set(unit.label),
        })
        .exec(txn)
        .await?;
        for (name, value) in unit.properties {
            let value = property_columns(value)?;
            unit_properties::Entity::insert(unit_properties::ActiveModel {
                domain_version_id: Set(key.clone()),
                unit_id: Set(unit_id.clone()),
                name: Set(name),
                value_kind: Set(value.value_kind),
                boolean_value: Set(value.boolean_value),
                integer_value: Set(value.integer_value),
                number_value: Set(value.number_value),
                text_value: Set(value.text_value),
                structured_json: Set(value.structured_json),
            })
            .exec(txn)
            .await?;
        }
    }

    for (_, relation) in snapshot.relations {
        let relation_id = relation.id.0.clone();
        relations::Entity::insert(relations::ActiveModel {
            domain_version_id: Set(key.clone()),
            id: Set(relation_id.clone()),
            source_unit_id: Set(relation.source.0),
            target_unit_id: Set(relation.target.0),
            kind: Set(relation.kind),
        })
        .exec(txn)
        .await?;
        for (name, value) in relation.properties {
            let value = property_columns(value)?;
            relation_properties::Entity::insert(relation_properties::ActiveModel {
                domain_version_id: Set(key.clone()),
                relation_id: Set(relation_id.clone()),
                name: Set(name),
                value_kind: Set(value.value_kind),
                boolean_value: Set(value.boolean_value),
                integer_value: Set(value.integer_value),
                number_value: Set(value.number_value),
                text_value: Set(value.text_value),
                structured_json: Set(value.structured_json),
            })
            .exec(txn)
            .await?;
        }
    }
    Ok(())
}

#[async_trait]
impl DomainWriter for SeaOrmDomainRepository {
    async fn insert_domain_version(&self, snapshot: DomainSnapshot) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        insert_snapshot(&txn, snapshot).await?;
        txn.commit().await?;
        Ok(())
    }
    async fn insert_measurement_frame(
        &self,
        domain_id: &DomainId,
        domain_version: &DomainVersion,
        frame: MeasurementFrame,
    ) -> StoreResult<()> {
        let txn = self.db.begin().await?;
        let stored_domain_version = domain_versions::Entity::find()
            .filter(domain_versions::Column::DomainId.eq(&domain_id.0))
            .filter(domain_versions::Column::Version.eq(&domain_version.0))
            .one(&txn)
            .await?
            .ok_or_else(|| StoreError::NotFound {
                path: format!("domain {} version {}", domain_id.0, domain_version.0),
            })?;

        if let Some(stored_frame) = measurement_frames::Entity::find_by_id(&frame.id.0)
            .one(&txn)
            .await?
        {
            if stored_frame.domain_id != domain_id.0 {
                return Err(StoreError::Conflict { path: frame.id.0 });
            }
        } else {
            measurement_frames::Entity::insert(measurement_frames::ActiveModel {
                id: Set(frame.id.0.clone()),
                domain_id: Set(domain_id.0.clone()),
                label: Set(None),
                created_at: Set(now()),
            })
            .exec(&txn)
            .await?;
        }

        let key = frame_version_key(&frame.id, &frame.version);
        if frame_versions::Entity::find_by_id(&key)
            .one(&txn)
            .await?
            .is_some()
        {
            return Err(StoreError::AlreadyExists { path: key });
        }
        frame_versions::Entity::insert(frame_versions::ActiveModel {
            id: Set(key.clone()),
            frame_id: Set(frame.id.0),
            version: Set(frame.version.0),
            domain_version_id: Set(stored_domain_version.id.clone()),
            predecessor_id: Set(None),
            created_at: Set(now()),
        })
        .exec(&txn)
        .await?;

        for (position, axis) in frame.axes.into_iter().enumerate() {
            let position = i32::try_from(position).map_err(|_| StoreError::InvalidRequest {
                message: "measurement frame has too many axes".into(),
            })?;
            frame_axes::Entity::insert(frame_axes::ActiveModel {
                frame_version_id: Set(key.clone()),
                domain_version_id: Set(stored_domain_version.id.clone()),
                position: Set(position),
                unit_id: Set(axis.unit.0),
                label: Set(axis.label),
            })
            .exec(&txn)
            .await?;
        }
        txn.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl DomainReader for SeaOrmDomainRepository {
    async fn get_domain_version(
        &self,
        domain_id: &DomainId,
        version: &DomainVersion,
    ) -> StoreResult<Option<DomainSnapshot>> {
        let Some(stored_version) = domain_versions::Entity::find()
            .filter(domain_versions::Column::DomainId.eq(&domain_id.0))
            .filter(domain_versions::Column::Version.eq(&version.0))
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };

        let unit_rows = units::Entity::find()
            .filter(units::Column::DomainVersionId.eq(&stored_version.id))
            .all(&self.db)
            .await?;
        let unit_property_rows = unit_properties::Entity::find()
            .filter(unit_properties::Column::DomainVersionId.eq(&stored_version.id))
            .all(&self.db)
            .await?;
        let relation_rows = relations::Entity::find()
            .filter(relations::Column::DomainVersionId.eq(&stored_version.id))
            .all(&self.db)
            .await?;
        let relation_property_rows = relation_properties::Entity::find()
            .filter(relation_properties::Column::DomainVersionId.eq(&stored_version.id))
            .all(&self.db)
            .await?;

        let mut unit_properties_by_id: BTreeMap<String, BTreeMap<String, PropertyValue>> =
            BTreeMap::new();
        for row in unit_property_rows {
            let value = parse_property(
                &row.value_kind,
                row.boolean_value,
                row.integer_value,
                row.number_value,
                row.text_value,
                row.structured_json,
            )?;
            unit_properties_by_id
                .entry(row.unit_id)
                .or_default()
                .insert(row.name, value);
        }

        let mut hydrated_units = BTreeMap::new();
        for row in unit_rows {
            let id = UnitId::new(row.id);
            hydrated_units.insert(
                id.clone(),
                Unit {
                    properties: unit_properties_by_id.remove(&id.0).unwrap_or_default(),
                    id,
                    kind: parse_unit_kind(&row.kind)?,
                    label: row.label,
                },
            );
        }

        let mut relation_properties_by_id: BTreeMap<String, BTreeMap<String, PropertyValue>> =
            BTreeMap::new();
        for row in relation_property_rows {
            let value = parse_property(
                &row.value_kind,
                row.boolean_value,
                row.integer_value,
                row.number_value,
                row.text_value,
                row.structured_json,
            )?;
            relation_properties_by_id
                .entry(row.relation_id)
                .or_default()
                .insert(row.name, value);
        }

        let mut hydrated_relations = BTreeMap::new();
        for row in relation_rows {
            let id = RelationId::new(row.id);
            hydrated_relations.insert(
                id.clone(),
                Relation {
                    properties: relation_properties_by_id.remove(&id.0).unwrap_or_default(),
                    id,
                    source: UnitId::new(row.source_unit_id),
                    target: UnitId::new(row.target_unit_id),
                    kind: row.kind,
                },
            );
        }

        Ok(Some(DomainSnapshot {
            id: DomainId::new(stored_version.domain_id),
            version: DomainVersion::new(stored_version.version),
            units: hydrated_units,
            relations: hydrated_relations,
        }))
    }
    async fn get_measurement_frame(
        &self,
        frame_id: &FrameId,
        version: &FrameVersion,
    ) -> StoreResult<Option<MeasurementFrame>> {
        let Some(stored) = frame_versions::Entity::find()
            .filter(frame_versions::Column::FrameId.eq(&frame_id.0))
            .filter(frame_versions::Column::Version.eq(&version.0))
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };
        let axes = frame_axes::Entity::find()
            .filter(frame_axes::Column::FrameVersionId.eq(&stored.id))
            .order_by_asc(frame_axes::Column::Position)
            .all(&self.db)
            .await?
            .into_iter()
            .map(|axis| FrameAxis {
                unit: UnitId::new(axis.unit_id),
                label: axis.label,
            })
            .collect();
        Ok(Some(MeasurementFrame {
            id: FrameId::new(stored.frame_id),
            version: FrameVersion::new(stored.version),
            axes,
        }))
    }
}
