//! Epistemic operation types, tracked inputs, and provenance.
//!
//! Derived values cannot be constructed or relabeled directly outside this
//! crate; operation-specific tokens must emit them.
//!
//! ```compile_fail
//! use std::marker::PhantomData;
//! use unclip_epistemic::{ops, Derived};
//!
//! let _ = Derived::<u32, ops::Calculation> {
//!     id: todo!(),
//!     value: 1,
//!     provenance: todo!(),
//!     operation: PhantomData,
//! };
//! ```
//!
//! ```compile_fail
//! use unclip_epistemic::{Calculated, Inferred};
//!
//! fn relabel(value: Inferred<u32>) -> Calculated<u32> {
//!     value
//! }
//! ```

#![forbid(unsafe_code)]

use std::{
    collections::BTreeSet,
    fmt,
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use semver::Version;
use serde::{Deserialize, Serialize};

/// Canonicalize and hash plugin parameters with a stable FNV-1a digest.
///
/// Object keys are sorted recursively, so insertion order does not affect the
/// result. The digest is for reproducibility checks, not cryptographic use.
pub fn hash_params(params: &serde_json::Value) -> ParameterHash {
    let mut canonical = String::new();
    write_canonical_json(params, &mut canonical);
    let hash = canonical
        .bytes()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    ParameterHash::new(format!("fnv1a64:{hash:016x}"))
}

fn write_canonical_json(value: &serde_json::Value, output: &mut String) {
    match value {
        serde_json::Value::Null => output.push_str("null"),
        serde_json::Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        serde_json::Value::Number(value) => output.push_str(&value.to_string()),
        serde_json::Value::String(value) => {
            output
                .push_str(&serde_json::to_string(value).expect("string serialization cannot fail"));
        }
        serde_json::Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output);
            }
            output.push(']');
        }
        serde_json::Value::Object(values) => {
            output.push('{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output
                    .push_str(&serde_json::to_string(key).expect("key serialization cannot fail"));
                output.push(':');
                write_canonical_json(value, output);
            }
            output.push('}');
        }
    }
}

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

impl ModelRef {
    /// Build the stable model selector stored in provenance.
    pub fn versioned(identity: impl AsRef<str>, version: impl AsRef<str>) -> Self {
        Self(format!("{}@{}", identity.as_ref(), version.as_ref()))
    }
}

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
    /// Track a value extracted from a derived aggregate under that aggregate's provenance id.
    pub fn from_derived<S, O: OperationKind>(source: &Derived<S, O>, value: T) -> Self {
        Self {
            id: source.id.clone(),
            value,
        }
    }

    /// Restore a tracked value using its persisted provenance identity.
    pub fn from_recorded(id: DerivedId, value: T) -> Self {
        Self { id, value }
    }

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
    emitted: AtomicUsize,
    operation: PhantomData<O>,
}

impl<O: OperationKind> EmitToken<O> {
    #[doc(hidden)]
    pub fn from_harness(metadata: EmitMetadata, dependencies: DependencyCollector) -> Self {
        Self {
            metadata,
            dependencies,
            emitted: AtomicUsize::new(0),
            operation: PhantomData,
        }
    }

    pub fn emit<T>(&self, value: T) -> Derived<T, O> {
        let sequence = self.emitted.fetch_add(1, Ordering::Relaxed);
        let id = if sequence == 0 {
            self.metadata.id.clone()
        } else {
            DerivedId::new(format!("{}#{sequence}", self.metadata.id))
        };
        let provenance = Provenance {
            operation: O::OPERATION,
            producer: self.metadata.producer.clone(),
            algorithm: self.metadata.algorithm.clone(),
            version: self.metadata.version.clone(),
            params: self.metadata.params.clone(),
            params_hash: self.metadata.params_hash.clone(),
            inputs: self.dependencies.snapshot(),
            source: self.metadata.source.clone(),
            timestamp: self.metadata.timestamp.clone(),
            domain_version: self.metadata.domain_version.clone(),
            frame_version: self.metadata.frame_version.clone(),
            model: self.metadata.model.clone(),
        };
        Derived {
            id,
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
    fn parameter_hash_is_recursive_and_order_independent() {
        let left = serde_json::json!({"z": 1, "nested": {"b": 2, "a": [3, 4]}});
        let right = serde_json::json!({"nested": {"a": [3, 4], "b": 2}, "z": 1});
        assert_eq!(hash_params(&left), hash_params(&right));
        assert_ne!(
            hash_params(&left),
            hash_params(&serde_json::json!({"z": 2}))
        );
    }

    #[test]
    fn one_token_emits_stable_unique_ids() {
        let token =
            CalculationToken::from_harness(metadata("measurement"), DependencyCollector::default());
        let first = token.emit(1);
        let second = token.emit(2);
        assert_eq!(first.id(), &DerivedId::new("measurement"));
        assert_eq!(second.id(), &DerivedId::new("measurement#1"));
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
