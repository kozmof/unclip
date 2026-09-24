//! Lazy cross-domain composition types.
//!
//! Product domains and their measurement frames are separate from ordinary
//! `DomainSnapshot` and `MeasurementFrame` values. They retain interaction
//! coordinates without copying either input domain's units or relations.

use serde::{Deserialize, Serialize};
use unclip_epistemic::{DerivedId, DomainVersion};

use crate::{DomainId, UnitId};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
        }
    };
}

string_id!(ProductDomainId);
string_id!(ProductDomainVersion);
string_id!(ProductFrameId);
string_id!(ProductFrameVersion);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductDomainInput {
    pub domain: DomainId,
    pub version: DomainVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductInteraction {
    pub left: UnitId,
    pub right: UnitId,
    #[serde(default)]
    pub observations: Vec<DerivedId>,
    #[serde(default)]
    pub requirements: Vec<DerivedId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductDomainSnapshot {
    pub id: ProductDomainId,
    pub version: ProductDomainVersion,
    pub left: ProductDomainInput,
    pub right: ProductDomainInput,
    pub interactions: Vec<ProductInteraction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductFrameAxis {
    pub left: UnitId,
    pub right: UnitId,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductMeasurementFrame {
    pub id: ProductFrameId,
    pub version: ProductFrameVersion,
    pub product: ProductDomainId,
    pub product_version: ProductDomainVersion,
    pub left: ProductDomainInput,
    pub right: ProductDomainInput,
    pub axes: Vec<ProductFrameAxis>,
}
