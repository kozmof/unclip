//! Versioned semantic domains and measurement coordinate frames.

#![forbid(unsafe_code)]

mod candidate;
mod product;
pub use candidate::{CandidateKind, CandidateProposal};
pub use product::{
    ProductDomainId, ProductDomainInput, ProductDomainSnapshot, ProductDomainVersion,
    ProductFrameAxis, ProductFrameId, ProductFrameVersion, ProductInteraction,
    ProductMeasurementFrame,
};

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use unclip_epistemic::{string_id, DomainVersion, FrameVersion};

string_id!(DomainId);
string_id!(UnitId);
string_id!(RelationId);
string_id!(FrameId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    AtomicMeaning,
    CompositeMeaning,
    SemanticRole,
    GraphMotif,
    Transformation,
    DynamicCoupling,
    LatentAxis,
    CrossDomainStructure,
}

/// One value of a domain unit's or relation's property.
///
/// # Canonical form
///
/// [`Self::Structured`] holds arbitrary JSON, which includes the scalars the
/// other four variants already express — so `Structured(json!(true))` and
/// `Boolean(true)` denote one value in two spellings. That matters because
/// equality here is structural and the engine's revision checks decide
/// acceptance by comparing property values: two spellings compare unequal, and
/// which one a caller holds depended on where it came from. The untagged
/// representation collapses `true` to `Boolean(true)`, while the store's
/// discriminated columns preserve whichever variant was written.
///
/// A value is **canonical** when `Structured` holds only an array, an object,
/// or null — never a bool, number, or string. [`Self::structured`] builds one
/// that way, [`Self::canonical`] converts one, and deserializing always yields
/// one. Construct the `Structured` variant directly only when the payload is
/// known to be composite.
///
/// # Non-finite numbers
///
/// JSON has no NaN or infinity, and `serde_json` writes both as `null` — so a
/// `Number(f64::NAN)` serialized and read back is `Structured(Null)`, a
/// different variant holding no number at all. The store already refuses a
/// non-finite numeric property; [`Self::serialize`] refuses one too, so the
/// two boundaries agree instead of one erroring and the other corrupting.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum PropertyValue {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    Text(String),
    Structured(serde_json::Value),
}

impl PropertyValue {
    /// Build a property from arbitrary JSON, in canonical form.
    ///
    /// A scalar payload becomes the variant that expresses it, so the result
    /// compares equal to the same value written any other way.
    pub fn structured(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Bool(value) => Self::Boolean(value),
            serde_json::Value::String(value) => Self::Text(value),
            serde_json::Value::Number(number) => match number.as_i64() {
                Some(value) => Self::Integer(value),
                // A non-integer JSON number is always finite, so this cannot
                // produce the non-finite value `serialize` refuses.
                None => match number.as_f64() {
                    Some(value) => Self::Number(value),
                    None => Self::Structured(serde_json::Value::Number(number)),
                },
            },
            composite => Self::Structured(composite),
        }
    }

    /// This value in canonical form. Already-canonical values are unchanged.
    #[must_use]
    pub fn canonical(self) -> Self {
        match self {
            Self::Structured(value) => Self::structured(value),
            scalar => scalar,
        }
    }

    /// Whether this value survives serialization unchanged.
    ///
    /// False only for a non-finite [`Self::Number`], which JSON cannot hold.
    pub fn is_representable(&self) -> bool {
        match self {
            Self::Number(value) => value.is_finite(),
            _ => true,
        }
    }
}

// Hand-written so a non-finite number fails loudly instead of being written as
// `null` and read back as a different variant. Everything else serializes
// exactly as the untagged derive would: as the bare scalar or JSON value, with
// no variant tag.
impl Serialize for PropertyValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Boolean(value) => serializer.serialize_bool(*value),
            Self::Integer(value) => serializer.serialize_i64(*value),
            Self::Number(value) => {
                if !value.is_finite() {
                    return Err(serde::ser::Error::custom(format!(
                        "numeric domain properties must be finite, got {value}"
                    )));
                }
                serializer.serialize_f64(*value)
            }
            Self::Text(value) => serializer.serialize_str(value),
            Self::Structured(value) => value.serialize(serializer),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub id: UnitId,
    pub kind: UnitKind,
    pub label: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub id: RelationId,
    pub source: UnitId,
    pub target: UnitId,
    pub kind: String,
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainSnapshot {
    pub id: DomainId,
    pub version: DomainVersion,
    pub units: BTreeMap<UnitId, Unit>,
    pub relations: BTreeMap<RelationId, Relation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameAxis {
    pub unit: UnitId,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementFrame {
    pub id: FrameId,
    pub version: FrameVersion,
    pub axes: Vec<FrameAxis>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every canonical property value survives a JSON round trip as itself.
    ///
    /// The untagged representation carries no variant tag, so a variant is
    /// recovered from the shape of the value alone. That is exact for the four
    /// scalars and for a composite `Structured`, and it is why `Structured`
    /// must not hold a scalar: nothing in the encoding could tell it from the
    /// scalar variant on the way back.
    #[test]
    fn canonical_property_values_round_trip_as_themselves() {
        for value in [
            PropertyValue::Boolean(true),
            PropertyValue::Boolean(false),
            PropertyValue::Integer(0),
            PropertyValue::Integer(-7),
            PropertyValue::Number(3.5),
            PropertyValue::Number(-0.25),
            PropertyValue::Text(String::new()),
            PropertyValue::Text("stable".into()),
            PropertyValue::Structured(serde_json::json!({"tags": ["a", "b"]})),
            PropertyValue::Structured(serde_json::json!([1, 2, 3])),
            PropertyValue::Structured(serde_json::Value::Null),
        ] {
            let encoded = serde_json::to_string(&value).expect("canonical values serialize");
            let decoded: PropertyValue =
                serde_json::from_str(&encoded).expect("canonical values deserialize");
            assert_eq!(decoded, value, "round trip changed {value:?} via {encoded}");
            assert!(value.is_representable());
        }
    }

    /// A scalar wrapped in `Structured` is the same value as the scalar, and
    /// canonicalizing says so. The engine's revision checks compare property
    /// values structurally, so the two spellings must not both exist: one is
    /// what a YAML import produces and the other is what a `structured`
    /// database row used to hydrate as.
    #[test]
    fn scalars_wrapped_in_structured_canonicalize_to_their_own_variant() {
        for (wrapped, canonical) in [
            (serde_json::json!(true), PropertyValue::Boolean(true)),
            (serde_json::json!(7), PropertyValue::Integer(7)),
            (serde_json::json!(2.5), PropertyValue::Number(2.5)),
            (serde_json::json!("x"), PropertyValue::Text("x".into())),
        ] {
            assert_eq!(PropertyValue::structured(wrapped.clone()), canonical);
            assert_eq!(
                PropertyValue::Structured(wrapped.clone()).canonical(),
                canonical
            );
            // Which is also what the wire form decodes to, so an uncanonical
            // value in memory and its own encoding disagree — the bug this
            // canonicalization removes.
            let decoded: PropertyValue =
                serde_json::from_str(&serde_json::to_string(&wrapped).unwrap()).unwrap();
            assert_eq!(decoded, canonical);
        }

        // Composites are already canonical and are left exactly as they are.
        let composite = serde_json::json!({"a": 1});
        assert_eq!(
            PropertyValue::structured(composite.clone()),
            PropertyValue::Structured(composite)
        );
    }

    /// JSON cannot hold NaN or infinity, and `serde_json` writes both as
    /// `null` — which reads back as `Structured(Null)`, a different variant
    /// holding no number. Serializing refuses instead, matching the store,
    /// which already rejects a non-finite numeric property on insert.
    #[test]
    fn non_finite_numbers_refuse_to_serialize_instead_of_becoming_null() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let property = PropertyValue::Number(value);
            assert!(!property.is_representable());
            let error = serde_json::to_string(&property)
                .expect_err("a non-finite property must not serialize");
            assert!(error.to_string().contains("finite"), "got: {error}");
        }
        assert!(PropertyValue::Number(0.0).is_representable());
    }

    #[test]
    fn domain_and_frame_versions_are_independent() {
        let domain = DomainSnapshot {
            id: DomainId::new("coffee"),
            version: DomainVersion::new("7"),
            units: BTreeMap::new(),
            relations: BTreeMap::new(),
        };
        let frame = MeasurementFrame {
            id: FrameId::new("coffee.general"),
            version: FrameVersion::new("2"),
            axes: Vec::new(),
        };
        assert_eq!(domain.version.0, "7");
        assert_eq!(frame.version.0, "2");
    }
}
