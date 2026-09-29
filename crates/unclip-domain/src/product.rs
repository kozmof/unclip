//! Lazy cross-domain composition types.
//!
//! Product domains and their measurement frames are separate from ordinary
//! `DomainSnapshot` and `MeasurementFrame` values. They retain interaction
//! coordinates without copying either input domain's units or relations.

use serde::{Deserialize, Serialize};
use unclip_epistemic::{string_id, DerivedId, DomainVersion};

use crate::{DomainId, UnitId};

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
