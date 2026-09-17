//! Epistemic operation types, tracked inputs, and provenance.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeSet,
    fmt,
    marker::PhantomData,
    sync::{Arc, Mutex},
};

use semver::Version;
use serde::{Deserialize, Serialize};

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

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(DerivedId);
string_id!(PluginId);
string_id!(DomainVersion);
string_id!(FrameVersion);
string_id!(SourceRef);
string_id!(Timestamp);
string_id!(ModelRef);
string_id!(ParameterHash);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Inferred,
    Calculated,
    Experimental,
    Interpreted,
}

pub trait OperationKind: private::Sealed + Send + Sync + 'static {
    const OPERATION: Operation;
}

pub mod ops {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Inference;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Calculation;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Experiment;
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Interpretation;
}

mod private {
    pub trait Sealed {}
    impl Sealed for super::ops::Inference {}
    impl Sealed for super::ops::Calculation {}
    impl Sealed for super::ops::Experiment {}
    impl Sealed for super::ops::Interpretation {}
}

impl OperationKind for ops::Inference {
    const OPERATION: Operation = Operation::Inferred;
}
impl OperationKind for ops::Calculation {
    const OPERATION: Operation = Operation::Calculated;
}
impl OperationKind for ops::Experiment {
    const OPERATION: Operation = Operation::Experimental;
}
impl OperationKind for ops::Interpretation {
    const OPERATION: Operation = Operation::Interpreted;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub operation: Operation,
    pub producer: PluginId,
    pub algorithm: String,
    pub version: Version,
    pub params: serde_json::Value,
    pub params_hash: ParameterHash,
    pub inputs: Vec<DerivedId>,
    pub source: Option<SourceRef>,
    pub timestamp: Timestamp,
    pub domain_version: Option<DomainVersion>,
    pub frame_version: Option<FrameVersion>,
    pub model: Option<ModelRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Derived<T, O: OperationKind> {
    id: DerivedId,
    value: T,
    provenance: Arc<Provenance>,
    operation: PhantomData<O>,
}

impl<T, O: OperationKind> Derived<T, O> {
    pub fn id(&self) -> &DerivedId {
        &self.id
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub fn into_value(self) -> T {
        self.value
    }
}

pub type Inferred<T> = Derived<T, ops::Inference>;
pub type Calculated<T> = Derived<T, ops::Calculation>;
pub type Experimental<T> = Derived<T, ops::Experiment>;
pub type Interpreted<T> = Derived<T, ops::Interpretation>;

#[derive(Debug, Clone)]
pub struct Tracked<T> {
    id: DerivedId,
    value: T,
}

impl<T, O: OperationKind> From<&Derived<T, O>> for Tracked<T>
where
    T: Clone,
{
    fn from(value: &Derived<T, O>) -> Self {
        Self {
            id: value.id.clone(),
            value: value.value.clone(),
        }
    }
}

impl<T> Tracked<T> {
    pub fn id(&self) -> &DerivedId {
        &self.id
    }
}

#[derive(Debug, Clone, Default)]
pub struct DependencyCollector(Arc<Mutex<BTreeSet<DerivedId>>>);

impl DependencyCollector {
    pub fn read<'a, T>(&self, input: &'a Tracked<T>) -> &'a T {
        self.0
            .lock()
            .expect("dependency collector poisoned")
            .insert(input.id.clone());
        &input.value
    }

    pub fn snapshot(&self) -> Vec<DerivedId> {
        self.0
            .lock()
            .expect("dependency collector poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub fn take(&self) -> Vec<DerivedId> {
        std::mem::take(&mut *self.0.lock().expect("dependency collector poisoned"))
            .into_iter()
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct EmitMetadata {
    pub id: DerivedId,
    pub producer: PluginId,
    pub algorithm: String,
    pub version: Version,
    pub params: serde_json::Value,
    pub params_hash: ParameterHash,
    pub source: Option<SourceRef>,
    pub timestamp: Timestamp,
    pub domain_version: Option<DomainVersion>,
    pub frame_version: Option<FrameVersion>,
    pub model: Option<ModelRef>,
}

#[derive(Debug)]
pub struct EmitToken<O: OperationKind> {
    metadata: EmitMetadata,
    dependencies: DependencyCollector,
    operation: PhantomData<O>,
}

impl<O: OperationKind> EmitToken<O> {
    #[doc(hidden)]
    pub fn from_harness(metadata: EmitMetadata, dependencies: DependencyCollector) -> Self {
        Self {
            metadata,
            dependencies,
            operation: PhantomData,
        }
    }

    pub fn emit<T>(self, value: T) -> Derived<T, O> {
        let metadata = self.metadata;
        let provenance = Provenance {
            operation: O::OPERATION,
            producer: metadata.producer,
            algorithm: metadata.algorithm,
            version: metadata.version,
            params: metadata.params,
            params_hash: metadata.params_hash,
            inputs: self.dependencies.take(),
            source: metadata.source,
            timestamp: metadata.timestamp,
            domain_version: metadata.domain_version,
            frame_version: metadata.frame_version,
            model: metadata.model,
        };
        Derived {
            id: metadata.id,
            value,
            provenance: Arc::new(provenance),
            operation: PhantomData,
        }
    }
}

pub type InferenceToken = EmitToken<ops::Inference>;
pub type CalculationToken = EmitToken<ops::Calculation>;
pub type ExperimentToken = EmitToken<ops::Experiment>;
pub type InterpretationToken = EmitToken<ops::Interpretation>;

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(id: &str) -> EmitMetadata {
        EmitMetadata {
            id: DerivedId::new(id),
            producer: PluginId::new("sensor.test"),
            algorithm: "test".into(),
            version: Version::new(0, 1, 0),
            params: serde_json::json!({}),
            params_hash: ParameterHash::new("hash"),
            source: None,
            timestamp: Timestamp::new("2026-09-17T00:00:00Z"),
            domain_version: None,
            frame_version: None,
            model: None,
        }
    }

    #[test]
    fn operation_and_dependencies_are_attached_by_token() {
        let source =
            InferenceToken::from_harness(metadata("source"), DependencyCollector::default())
                .emit(String::from("value"));
        let tracked = Tracked::from(&source);
        let dependencies = DependencyCollector::default();
        assert_eq!(dependencies.read(&tracked), "value");
        let result = CalculationToken::from_harness(metadata("result"), dependencies).emit(42);
        assert_eq!(result.provenance().operation, Operation::Calculated);
        assert_eq!(result.provenance().inputs, vec![DerivedId::new("source")]);
    }

    #[test]
    fn dependency_ids_are_sorted_and_deduplicated() {
        let a = Tracked {
            id: DerivedId::new("a"),
            value: 1,
        };
        let b = Tracked {
            id: DerivedId::new("b"),
            value: 2,
        };
        let collector = DependencyCollector::default();
        collector.read(&b);
        collector.read(&a);
        collector.read(&b);
        assert_eq!(
            collector.snapshot(),
            vec![DerivedId::new("a"), DerivedId::new("b")]
        );
    }
}
