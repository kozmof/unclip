use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;
use unclip_domain::{
    DomainId, DomainSnapshot, ProductDomainId, ProductDomainVersion, ProductInteraction, Relation,
    RelationId, Unit, UnitId, UnitKind,
};
use unclip_engine::Engine;
use unclip_epistemic::{DerivedId, DomainVersion, Operation, Timestamp, Tracked};

fn unit(id: &str) -> Unit {
    Unit {
        id: UnitId::new(id),
        kind: UnitKind::AtomicMeaning,
        label: Some(id.into()),
        properties: BTreeMap::new(),
    }
}

fn domain(id: &str, version: &str, units: &[&str]) -> DomainSnapshot {
    DomainSnapshot {
        id: DomainId::new(id),
        version: DomainVersion::new(version),
        units: units
            .iter()
            .map(|id| (UnitId::new(*id), unit(id)))
            .collect(),
        relations: BTreeMap::new(),
    }
}

fn interaction(
    id: &str,
    left: &str,
    right: &str,
    observations: &[&str],
    requirements: &[&str],
) -> Tracked<ProductInteraction> {
    Tracked::from_recorded(
        DerivedId::new(id),
        ProductInteraction {
            left: UnitId::new(left),
            right: UnitId::new(right),
            observations: observations.iter().map(|id| DerivedId::new(*id)).collect(),
            requirements: requirements.iter().map(|id| DerivedId::new(*id)).collect(),
        },
    )
}

#[test]
fn product_domain_materializes_only_observed_or_required_pairs() {
    let left_value = domain("coffee", "7", &["presentation", "social"]);
    let right_value = domain("photo", "3", &["composition", "sharing"]);
    let left = Tracked::from_recorded(DerivedId::new("coffee@7"), left_value.clone());
    let right = Tracked::from_recorded(DerivedId::new("photo@3"), right_value.clone());
    let observed = interaction(
        "observed-pair",
        "presentation",
        "composition",
        &["observation-1", "observation-2"],
        &[],
    );
    let required = interaction(
        "required-pair",
        "social",
        "sharing",
        &[],
        &["measurement-requirement"],
    );

    let engine = Engine::with_builtins().unwrap();
    let result = engine
        .materialize_product_domain(
            &left,
            &right,
            &[required.clone(), observed.clone()],
            ProductDomainId::new("coffee-x-photo"),
            ProductDomainVersion::new("1"),
            "coffee-photo",
            Timestamp::new("now"),
        )
        .unwrap();
    let replay = engine
        .materialize_product_domain(
            &left,
            &right,
            &[observed, required],
            ProductDomainId::new("coffee-x-photo"),
            ProductDomainVersion::new("1"),
            "coffee-photo",
            Timestamp::new("now"),
        )
        .unwrap();

    assert_eq!(result, replay);
    assert_eq!(result.provenance().operation, Operation::Calculated);
    assert_eq!(result.value().left.domain, DomainId::new("coffee"));
    assert_eq!(result.value().left.version, DomainVersion::new("7"));
    assert_eq!(result.value().right.domain, DomainId::new("photo"));
    assert_eq!(result.value().right.version, DomainVersion::new("3"));
    assert_eq!(result.value().interactions.len(), 2);
    assert_eq!(
        result
            .value()
            .interactions
            .iter()
            .map(|interaction| (interaction.left.0.clone(), interaction.right.0.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("presentation".into(), "composition".into()),
            ("social".into(), "sharing".into()),
        ]
    );
    assert_eq!(
        result
            .value()
            .interactions
            .iter()
            .flat_map(|interaction| interaction.observations.iter())
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            DerivedId::new("observation-1"),
            DerivedId::new("observation-2")
        ]
    );
    assert_eq!(
        result
            .provenance()
            .inputs
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            DerivedId::new("coffee@7"),
            DerivedId::new("photo@3"),
            DerivedId::new("observed-pair"),
            DerivedId::new("required-pair"),
        ])
    );
    let encoded = serde_json::to_value(result.value()).unwrap();
    assert_eq!(encoded["id"], "coffee-x-photo");
    assert_eq!(encoded["version"], "1");
    assert!(encoded.get("units").is_none());
    assert!(encoded.get("relations").is_none());
    assert_eq!(
        serde_json::from_value::<unclip_domain::ProductDomainSnapshot>(encoded).unwrap(),
        result.value().clone()
    );
}

#[test]
fn product_domain_can_remain_empty_without_creating_a_cartesian_union() {
    let result = Engine::with_builtins()
        .unwrap()
        .materialize_product_domain(
            &Tracked::from_recorded(DerivedId::new("left"), domain("left", "1", &["a", "b"])),
            &Tracked::from_recorded(DerivedId::new("right"), domain("right", "2", &["x", "y"])),
            &[],
            ProductDomainId::new("left-x-right"),
            ProductDomainVersion::new("empty"),
            "empty-product",
            Timestamp::new("now"),
        )
        .unwrap();
    assert!(result.value().interactions.is_empty());
    assert_eq!(
        result.provenance().params["materialized_interactions"],
        json!(0)
    );
}

#[test]
fn product_domain_rejects_unbacked_ambiguous_or_invalid_pairs() {
    let engine = Engine::with_builtins().unwrap();
    let left = Tracked::from_recorded(
        DerivedId::new("left-input"),
        domain("left", "1", &["a", "b"]),
    );
    let right = Tracked::from_recorded(
        DerivedId::new("right-input"),
        domain("right", "1", &["x", "y"]),
    );
    let run = |interactions: &[Tracked<ProductInteraction>]| {
        engine.materialize_product_domain(
            &left,
            &right,
            interactions,
            ProductDomainId::new("left-x-right"),
            ProductDomainVersion::new("1"),
            "product",
            Timestamp::new("now"),
        )
    };

    assert!(run(&[interaction("empty", "a", "x", &[], &[])]).is_err());
    assert!(run(&[interaction("missing", "missing", "x", &["o1"], &[])]).is_err());
    assert!(run(&[interaction("unsorted", "a", "x", &["o2", "o1"], &[])]).is_err());
    assert!(run(&[interaction(
        "overlap",
        "a",
        "x",
        &["same-evidence"],
        &["same-evidence"],
    )])
    .is_err());
    assert!(run(&[
        interaction("pair-1", "a", "x", &["o1"], &[]),
        interaction("pair-2", "a", "x", &[], &["required"]),
    ])
    .is_err());

    let duplicate_id = [
        interaction("same-evidence", "a", "x", &["o1"], &[]),
        interaction("same-evidence", "b", "y", &["o2"], &[]),
    ];
    assert!(run(&duplicate_id).is_err());

    let same_domain = Tracked::from_recorded(
        DerivedId::new("same-right-input"),
        domain("left", "2", &["x"]),
    );
    assert!(engine
        .materialize_product_domain(
            &left,
            &same_domain,
            &[],
            ProductDomainId::new("invalid"),
            ProductDomainVersion::new("1"),
            "same-domain",
            Timestamp::new("now"),
        )
        .is_err());

    let mut dangling = domain("dangling", "1", &["x"]);
    dangling.relations.insert(
        RelationId::new("broken"),
        Relation {
            id: RelationId::new("broken"),
            source: UnitId::new("x"),
            target: UnitId::new("missing"),
            kind: "broken".into(),
            properties: BTreeMap::new(),
        },
    );
    assert!(engine
        .materialize_product_domain(
            &left,
            &Tracked::from_recorded(DerivedId::new("dangling-input"), dangling),
            &[],
            ProductDomainId::new("invalid"),
            ProductDomainVersion::new("1"),
            "dangling",
            Timestamp::new("now"),
        )
        .is_err());
}
