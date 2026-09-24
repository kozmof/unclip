//! Lazy cross-domain composition types.
//!
//! A product domain records interactions between two immutable domains. It does
//! not copy either domain's units or relations and cannot be confused with an
//! ordinary `DomainSnapshot` union.

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
