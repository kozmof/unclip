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
//!     value: todo!(),
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
//!
//! A tracked input's payload cannot be taken without going through a
//! [`DependencyCollector`], so every read lands in provenance.
//!
//! ```compile_fail
//! use unclip_epistemic::{DerivedId, Tracked};
//!
//! let input = Tracked::from_recorded(DerivedId::new("input"), 1_u32);
//! let _ = input.shared();
//! ```

#![forbid(unsafe_code)]

use std::{
    collections::BTreeSet,
    marker::PhantomData,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, PoisonError,
    },
};

use semver::Version;
use serde::{Deserialize, Serialize};

/// One plugin's parameters for one run, shared rather than copied.
///
/// A run's parameters are configured once and then read by every emission the
/// plugin makes under them — once per source, candidate, or comparator in the
/// loops that drive a stage. [`EmitMetadata::new`] takes one of these and
/// [`Provenance`] keeps it, so the tree is allocated once and aliased from then
/// on. It serializes exactly as the bare tree does.
pub type SharedParams = Arc<serde_json::Value>;

/// Per-plugin parameters for one run, keyed by the plugin they configure.
pub type PluginParams = std::collections::BTreeMap<PluginId, SharedParams>;

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

/// Declare a transparent string newtype used as a stable identifier.
///
/// Identifiers across the workspace share one shape: an ordered, hashable
/// wrapper that serializes as a bare string and displays as its contents.
/// Declaring them through one macro keeps that contract identical in every
/// crate rather than re-deriving it per module.
///
/// The payload is an [`Arc<str>`](std::sync::Arc), so copying an identifier is
/// a refcount bump rather than an allocation. Identifiers are copied once per
/// sample, axis, unit and candidate on every path in the workspace, which is
/// what made an owned `String` here the largest remaining source of
/// duplication. The field is private: read it with `as_str`,
/// `AsRef<str>`, `Deref<Target = str>` or `Display`, and reach for an owned
/// `String` only at a boundary that demands one.
///
/// ```
/// unclip_epistemic::string_id!(
///     /// A branch coordinate.
///     ExampleId
/// );
///
/// let id = ExampleId::new("example");
/// assert_eq!(id.as_str(), "example");
/// assert_eq!(id.to_string(), "example");
/// assert_eq!(serde_json::to_string(&id).unwrap(), "\"example\"");
///
/// // Copies share one allocation rather than allocating again.
/// let copy = id.clone();
/// assert!(std::ptr::eq(id.as_str(), copy.as_str()));
/// ```
#[macro_export]
macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash,
            ::serde::Serialize, ::serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(::std::sync::Arc<str>);

        impl $name {
            pub fn new(value: impl Into<Self>) -> Self {
                value.into()
            }

            /// Borrow the identifier's text.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Share the identifier's allocation as a bare string handle.
            pub fn as_shared(&self) -> ::std::sync::Arc<str> {
                ::std::sync::Arc::clone(&self.0)
            }
        }

        impl ::std::convert::From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(::std::sync::Arc::from(value))
            }
        }

        impl ::std::convert::From<String> for $name {
            fn from(value: String) -> Self {
                Self(::std::sync::Arc::from(value))
            }
        }

        impl ::std::convert::From<&String> for $name {
            fn from(value: &String) -> Self {
                Self(::std::sync::Arc::from(value.as_str()))
            }
        }

        impl ::std::convert::From<::std::borrow::Cow<'_, str>> for $name {
            fn from(value: ::std::borrow::Cow<'_, str>) -> Self {
                Self(::std::sync::Arc::from(value))
            }
        }

        impl ::std::convert::From<::std::sync::Arc<str>> for $name {
            fn from(value: ::std::sync::Arc<str>) -> Self {
                Self(value)
            }
        }

        impl ::std::convert::From<&$name> for $name {
            fn from(value: &$name) -> Self {
                value.clone()
            }
        }

        impl ::std::convert::From<$name> for ::std::string::String {
            fn from(value: $name) -> Self {
                value.as_str().to_owned()
            }
        }

        impl ::std::convert::AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl ::std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::cmp::PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                &*self.0 == other
            }
        }

        impl ::std::cmp::PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                &*self.0 == *other
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
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
        Self(format!("{}@{}", identity.as_ref(), version.as_ref()).into())
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
    pub algorithm: Arc<str>,
    pub version: Version,
    /// Shared with the run configuration these parameters came from, with the
    /// token that emitted this value, and with every other value that token
    /// emitted. One run configures a plugin once and then emits under that
    /// configuration repeatedly — per source, candidate, or comparator — and one
    /// token routinely emits several results from one analysis; copying an
    /// arbitrary parameter tree at any of those hops bought nothing. It
    /// serializes exactly as the bare tree does.
    pub params: SharedParams,
    pub params_hash: ParameterHash,
    pub inputs: Vec<DerivedId>,
    pub source: Option<SourceRef>,
    pub timestamp: Timestamp,
    pub domain_version: Option<DomainVersion>,
    pub frame_version: Option<FrameVersion>,
    pub model: Option<ModelRef>,
}

/// A value produced by one epistemic operation, together with its provenance.
///
/// The payload is held behind an [`Arc`] rather than inline. A derived value is
/// immutable once emitted, and the same payload is routinely tracked as an
/// input, stored, compared, and reported within one run; owning it inline made
/// each of those hops deep-copy an observation, snapshot, or profile. Sharing
/// the allocation turns them into a refcount bump, and lets a `Derived` be
/// cloned even when its payload is not [`Clone`].
pub struct Derived<T: ?Sized, O: OperationKind> {
    id: DerivedId,
    value: Arc<T>,
    provenance: Arc<Provenance>,
    operation: PhantomData<O>,
}

// These are written out rather than derived because `derive` bounds every impl
// on `T`: cloning or comparing a `Derived` would then demand a clonable or
// comparable payload even where the payload is only shared, never copied.
impl<T: ?Sized, O: OperationKind> Clone for Derived<T, O> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            value: Arc::clone(&self.value),
            provenance: Arc::clone(&self.provenance),
            operation: PhantomData,
        }
    }
}

impl<T: std::fmt::Debug + ?Sized, O: OperationKind> std::fmt::Debug for Derived<T, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Derived")
            .field("id", &self.id)
            .field("value", &self.value)
            .field("provenance", &self.provenance)
            .finish()
    }
}

impl<T: PartialEq + ?Sized, O: OperationKind> PartialEq for Derived<T, O> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.provenance == other.provenance
            && (Arc::ptr_eq(&self.value, &other.value) || self.value == other.value)
    }
}

impl<T: Eq + ?Sized, O: OperationKind> Eq for Derived<T, O> {}

impl<T: ?Sized, O: OperationKind> Derived<T, O> {
    pub fn id(&self) -> &DerivedId {
        &self.id
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// Share this payload without copying it.
    ///
    /// The result aliases the same allocation, so a caller that needs the value
    /// to outlive this borrow no longer has to deep-copy it.
    pub fn shared(&self) -> Arc<T> {
        Arc::clone(&self.value)
    }

    /// Take the shared payload, dropping the provenance around it.
    pub fn into_shared(self) -> Arc<T> {
        self.value
    }

    /// Share this value's provenance without copying it.
    ///
    /// Provenance carries the emitting call's whole parameter tree, so a
    /// consumer that needs to keep or persist it should alias the record
    /// rather than duplicate it.
    pub fn shared_provenance(&self) -> Arc<Provenance> {
        Arc::clone(&self.provenance)
    }
}

impl<T: Clone, O: OperationKind> Derived<T, O> {
    /// Take the payload by value, copying it only if it is still shared.
    ///
    /// Where this is the last handle — a value emitted and immediately consumed
    /// — the allocation is unwrapped and nothing is copied.
    pub fn into_value(self) -> T {
        Arc::try_unwrap(self.value).unwrap_or_else(|shared| (*shared).clone())
    }
}

pub type Inferred<T> = Derived<T, ops::Inference>;
pub type Calculated<T> = Derived<T, ops::Calculation>;
pub type Experimental<T> = Derived<T, ops::Experiment>;
pub type Interpreted<T> = Derived<T, ops::Interpretation>;

/// One input to an operation, carrying the provenance identity to record for it.
///
/// As with [`Derived`], the payload is shared rather than owned inline.
/// Tracking a derived value as an input is the most common hop in the
/// workspace, and it must not cost a copy of the payload.
pub struct Tracked<T: ?Sized> {
    id: DerivedId,
    value: Arc<T>,
    operation: Option<Operation>,
}

impl<T: ?Sized> Clone for Tracked<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            value: Arc::clone(&self.value),
            operation: self.operation,
        }
    }
}

impl<T: std::fmt::Debug + ?Sized> std::fmt::Debug for Tracked<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracked")
            .field("id", &self.id)
            .field("value", &self.value)
            .field("operation", &self.operation)
            .finish()
    }
}

impl<T: PartialEq + ?Sized> PartialEq for Tracked<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.operation == other.operation
            && (Arc::ptr_eq(&self.value, &other.value) || self.value == other.value)
    }
}

impl<T: ?Sized, O: OperationKind> From<&Derived<T, O>> for Tracked<T> {
    fn from(value: &Derived<T, O>) -> Self {
        Self {
            id: value.id.clone(),
            value: Arc::clone(&value.value),
            operation: Some(O::OPERATION),
        }
    }
}

impl<T: ?Sized> Tracked<T> {
    /// Track an already-shared payload under a known identity and operation.
    ///
    /// This is the constructor to reach for when the payload is reachable as an
    /// [`Arc`] already — from [`Derived::shared`], from another `Tracked`, or
    /// from a record that stores it shared. The others below take the payload
    /// by value and allocate; this one only bumps a refcount.
    pub fn from_shared(id: DerivedId, value: Arc<T>, operation: Option<Operation>) -> Self {
        Self {
            id,
            value,
            operation,
        }
    }

    /// Share this payload without copying it.
    ///
    /// Test-only: outside this crate a tracked payload is reachable only
    /// through [`DependencyCollector`], so every read lands in provenance.
    #[cfg(test)]
    pub(crate) fn shared(&self) -> Arc<T> {
        Arc::clone(&self.value)
    }

    pub fn id(&self) -> &DerivedId {
        &self.id
    }

    /// Operation retained from an in-memory derived value. Persisted values use
    /// their type-specific repository as the trusted operation boundary.
    pub fn operation(&self) -> Option<Operation> {
        self.operation
    }
}

impl<T: ?Sized> Tracked<T> {
    /// Track a shared payload extracted from a derived aggregate under that
    /// aggregate's provenance id, without copying the payload.
    ///
    /// The same identity and operation as [`Self::from_derived`]; use this
    /// wherever the aggregate already holds the part being tracked behind an
    /// [`Arc`], so re-tracking it costs nothing.
    pub fn from_derived_shared<S: ?Sized, O: OperationKind>(
        source: &Derived<S, O>,
        value: Arc<T>,
    ) -> Self {
        Self {
            id: source.id.clone(),
            value,
            operation: Some(O::OPERATION),
        }
    }
}

impl<T> Tracked<T> {
    /// Track a value extracted from a derived aggregate under that aggregate's provenance id.
    pub fn from_derived<S: ?Sized, O: OperationKind>(source: &Derived<S, O>, value: T) -> Self {
        Self::from_derived_shared(source, Arc::new(value))
    }

    /// Restore a tracked value whose operation is not known here.
    ///
    /// The result carries no operation, so a gate such as the engine's
    /// calculated-evidence check cannot verify it and must accept it on trust.
    /// Prefer [`Self::from_calculated`] or [`Self::from_inferred`] whenever the
    /// table the value was read from establishes which operation produced it —
    /// which, for every first-party repository, it does.
    pub fn from_recorded(id: DerivedId, value: T) -> Self {
        Self {
            id,
            value: Arc::new(value),
            operation: None,
        }
    }

    /// Restore a value the caller knows was produced by calculation.
    ///
    /// Use this when the row's table admits only calculated values — a stored
    /// measurement, profile, or empirical structure. Naming the operation is
    /// what lets a downstream gate reject an interpreted or inferred value
    /// instead of accepting an unlabeled one.
    pub fn from_calculated(id: DerivedId, value: T) -> Self {
        Self {
            id,
            value: Arc::new(value),
            operation: Some(Operation::Calculated),
        }
    }

    /// Restore a value the caller knows was produced by inference.
    ///
    /// Observations, alignments, and rankings replayed from a recorded run are
    /// inference products; labeling them as such keeps a calculated-evidence
    /// gate from silently accepting them.
    pub fn from_inferred(id: DerivedId, value: T) -> Self {
        Self {
            id,
            value: Arc::new(value),
            operation: Some(Operation::Inferred),
        }
    }

    /// Restore a value under the operation its stored provenance records.
    ///
    /// The three constructors above ask the caller to *name* the operation,
    /// which is sound only where the source table settles the question. Where a
    /// table admits rows from more than one path — `empirical_structures` takes
    /// both calculated and imported structures — naming it is a guess, and the
    /// two wrong answers fail in opposite directions: `from_calculated` lets an
    /// interpreted row through a calculated-evidence gate, and `from_recorded`
    /// makes that gate inert.
    ///
    /// This constructor takes the operation as read rather than as claimed, so
    /// a caller holding the row's provenance can pass the recorded fact and let
    /// the gate check it. `None` still means "no claim", for the import and
    /// legacy rows that genuinely have none.
    pub fn from_stored(id: DerivedId, value: T, operation: Option<Operation>) -> Self {
        Self::from_shared(id, Arc::new(value), operation)
    }
}

#[derive(Debug, Clone, Default)]
pub struct DependencyCollector(Arc<Mutex<BTreeSet<DerivedId>>>);

impl DependencyCollector {
    /// Lock the input set, recovering its contents if the lock was poisoned.
    ///
    /// Poisoning means another thread panicked while holding this lock. The set
    /// it guards is append-only and every insert completes under the lock, so a
    /// panic cannot leave it half-written; the worst a poisoned lock implies
    /// here is that one input was not recorded. Recovering is therefore
    /// strictly better than propagating: panicking again while an earlier panic
    /// unwinds aborts the process, and it would abort inside provenance
    /// collection — the one place a failing run still needs to be able to
    /// report what it had read.
    fn guard(&self) -> std::sync::MutexGuard<'_, BTreeSet<DerivedId>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn read<'a, T: ?Sized>(&self, input: &'a Tracked<T>) -> &'a T {
        self.record(&input.id);
        &input.value
    }

    /// Read a derived value and record its identity without copying its payload.
    pub fn read_derived<'a, T: ?Sized, O: OperationKind>(&self, input: &'a Derived<T, O>) -> &'a T {
        self.record(&input.id);
        &input.value
    }

    /// Read a tracked input and take a shared handle to its payload.
    ///
    /// For callers that must keep the value past the borrow: the handle aliases
    /// the tracked allocation instead of copying it.
    pub fn read_shared<T: ?Sized>(&self, input: &Tracked<T>) -> Arc<T> {
        self.record(&input.id);
        Arc::clone(&input.value)
    }

    /// Read a derived value and take a shared handle to its payload.
    pub fn read_derived_shared<T: ?Sized, O: OperationKind>(
        &self,
        input: &Derived<T, O>,
    ) -> Arc<T> {
        self.record(&input.id);
        Arc::clone(&input.value)
    }

    fn record(&self, id: &DerivedId) {
        let mut inputs = self.guard();
        if !inputs.contains(id) {
            inputs.insert(id.clone());
        }
    }

    pub fn snapshot(&self) -> Vec<DerivedId> {
        self.guard().iter().cloned().collect()
    }

    /// Whether `id` has been recorded as an input, without copying the set.
    pub fn contains(&self, id: &DerivedId) -> bool {
        self.guard().contains(id)
    }

    /// Copy the recorded inputs as a set.
    ///
    /// For a caller that needs set semantics over the inputs; collecting
    /// [`Self::snapshot`] into a set would copy every identity twice.
    pub fn to_set(&self) -> BTreeSet<DerivedId> {
        self.guard().clone()
    }

    pub fn take(&self) -> Vec<DerivedId> {
        std::mem::take(&mut *self.guard()).into_iter().collect()
    }
}

#[derive(Debug, Clone)]
pub struct EmitMetadata {
    pub id: DerivedId,
    pub producer: PluginId,
    pub algorithm: Arc<str>,
    pub version: Version,
    pub params: SharedParams,
    pub params_hash: ParameterHash,
    pub source: Option<SourceRef>,
    pub timestamp: Timestamp,
    pub domain_version: Option<DomainVersion>,
    pub frame_version: Option<FrameVersion>,
    pub model: Option<ModelRef>,
}

impl EmitMetadata {
    /// The five fields every emission must supply, with the optional
    /// provenance qualifiers left unset.
    ///
    /// `params_hash` is derived from `params` rather than accepted separately:
    /// a caller that passes the two independently can pass a hash of something
    /// else, which would make the recorded provenance unverifiable. Callers
    /// that record a hash of different bytes than they record as parameters
    /// have to say so by assigning the field.
    ///
    /// `algorithm` defaults to the producer's id, which is what every
    /// first-party emission uses; override the field for a plugin that runs
    /// more than one named algorithm.
    ///
    /// The parameters are taken as [`SharedParams`]: a caller holding the run's
    /// configuration hands over a handle to it, and one that built a tree for
    /// this emission alone moves it in. Either way nothing is copied, where
    /// this used to deep-copy the whole tree once per token.
    pub fn new(
        id: DerivedId,
        producer: PluginId,
        version: Version,
        params: impl Into<SharedParams>,
        timestamp: Timestamp,
    ) -> Self {
        let params = params.into();
        Self {
            id,
            algorithm: producer.as_shared(),
            producer,
            version,
            params_hash: hash_params(&params),
            params,
            source: None,
            timestamp,
            domain_version: None,
            frame_version: None,
            model: None,
        }
    }

    // The qualifiers take `impl Into<Option<_>>` so a caller can pass either the
    // value or an already-optional one. Several callers forward a qualifier read
    // straight off an input's provenance, where it is already `Option`, and a
    // value-only signature would make each of those re-wrap it.

    /// Record the external source this value was derived from.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<Option<SourceRef>>) -> Self {
        self.source = source.into();
        self
    }

    /// Record the domain version this value was calculated against.
    #[must_use]
    pub fn with_domain_version(mut self, version: impl Into<Option<DomainVersion>>) -> Self {
        self.domain_version = version.into();
        self
    }

    /// Record the measurement-frame version this value was calculated against.
    #[must_use]
    pub fn with_frame_version(mut self, version: impl Into<Option<FrameVersion>>) -> Self {
        self.frame_version = version.into();
        self
    }

    /// Record the model selector that produced this value, when one did.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<Option<ModelRef>>) -> Self {
        self.model = model.into();
        self
    }

    /// Name the algorithm separately from the producing plugin.
    #[must_use]
    pub fn with_algorithm(mut self, algorithm: impl Into<Arc<str>>) -> Self {
        self.algorithm = algorithm.into();
        self
    }
}

#[derive(Debug)]
pub struct EmitToken<O: OperationKind> {
    metadata: EmitMetadata,
    dependencies: DependencyCollector,
    emitted: AtomicUsize,
    operation: PhantomData<O>,
}

impl<O: OperationKind> EmitToken<O> {
    /// Mint a token. Meant for the engine harness only.
    ///
    /// Rust cannot restrict a public function to one downstream crate, so a
    /// plugin can call this too. The engine therefore checks the producer of
    /// every value a plugin returns against the plugin it invoked, and rejects
    /// a mismatch as `PluginError::ForeignProducer`.
    #[doc(hidden)]
    pub fn from_harness(metadata: EmitMetadata, dependencies: DependencyCollector) -> Self {
        Self {
            metadata,
            dependencies,
            emitted: AtomicUsize::new(0),
            operation: PhantomData,
        }
    }

    /// Emit one derived value under this token's operation and provenance.
    ///
    /// # Provenance across several emissions
    ///
    /// One token may emit more than once, and each emission records **every**
    /// input read through the token's collector so far — not the inputs read
    /// since the previous emission. So the first of two emissions can omit an
    /// input the plugin reads afterwards, and both carry the union at the point
    /// each was emitted.
    ///
    /// This is deliberate, and per-emission deltas would be wrong rather than
    /// more precise. A sensor that derives several measurements from one pass
    /// over its evidence — `sensor.residual` emits `unmatched_units` and
    /// `unexplained_relations` from a single analysis — genuinely derives both
    /// from all of it, and splitting the reads by emission order would credit
    /// each with whichever fragment happened to be read last. The union is
    /// therefore the sound reading; it is imprecise only for a plugin that reads
    /// disjoint evidence per output, and such a plugin should take a token per
    /// output instead, which is what makes the distinction expressible.
    ///
    /// Provenance is consequently a superset of what a value strictly depends
    /// on. Verification relies on that direction: a missing input breaks a
    /// replay, an extra one does not.
    pub fn emit<T>(&self, value: T) -> Derived<T, O> {
        self.emit_shared(Arc::new(value))
    }

    /// Emit a value whose payload is already shared, without copying it.
    ///
    /// A stage that hands one payload to both its emitted result and a
    /// downstream input reaches for this, so only one allocation exists for it.
    /// It records an emission exactly as [`Self::emit`] does.
    pub fn emit_shared<T: ?Sized>(&self, value: Arc<T>) -> Derived<T, O> {
        let sequence = self.emitted.fetch_add(1, Ordering::Relaxed);
        let id = if sequence == 0 {
            self.metadata.id.clone()
        } else {
            DerivedId::new(format!("{}#{sequence}", self.metadata.id))
        };
        let provenance = Provenance {
            operation: O::OPERATION,
            producer: self.metadata.producer.clone(),
            algorithm: Arc::clone(&self.metadata.algorithm),
            version: self.metadata.version.clone(),
            params: Arc::clone(&self.metadata.params),
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
        EmitMetadata::new(
            DerivedId::new(id),
            PluginId::new("sensor.test"),
            Version::new(0, 1, 0),
            serde_json::json!({}),
            Timestamp::new("2026-09-17T00:00:00Z"),
        )
        .with_algorithm("test")
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
    fn derived_reads_borrow_non_clone_payloads_and_deduplicate_dependencies() {
        struct Payload(Vec<u8>);
        let source =
            InferenceToken::from_harness(metadata("source"), DependencyCollector::default())
                .emit(Payload(vec![1, 2, 3]));
        let dependencies = DependencyCollector::default();
        let value = dependencies.read_derived(&source);
        assert!(std::ptr::eq(value, source.value()));
        assert_eq!(value.0, [1, 2, 3]);
        dependencies.read_derived(&source);
        let tracked = Tracked::from_derived(&source, source.value());
        dependencies.read(&tracked);
        let result = CalculationToken::from_harness(metadata("result"), dependencies).emit(());
        assert_eq!(result.provenance().inputs, vec![DerivedId::new("source")]);
    }

    /// The point of holding payloads behind an `Arc`: tracking, cloning, and
    /// re-emitting a derived value must alias one allocation rather than deep-
    /// copy the payload at each hop. A regression here is silent — everything
    /// still compiles and every value still compares equal — so it is pinned
    /// by identity, not by contents.
    #[test]
    fn tracking_and_cloning_a_derived_value_share_one_payload() {
        let source =
            InferenceToken::from_harness(metadata("source"), DependencyCollector::default())
                .emit(String::from("payload"));
        let tracked = Tracked::from(&source);
        assert!(Arc::ptr_eq(&source.shared(), &tracked.shared()));

        let dependencies = DependencyCollector::default();
        assert!(std::ptr::eq(dependencies.read(&tracked), source.value()));
        assert!(Arc::ptr_eq(
            &dependencies.read_shared(&tracked),
            &source.shared()
        ));
        let copied_tracked = tracked.clone();
        assert!(Arc::ptr_eq(&copied_tracked.shared(), &source.shared()));
        let copied_source = source.clone();
        assert!(Arc::ptr_eq(&copied_source.into_shared(), &source.shared()));

        // Re-emitting under a new identity keeps the payload where it is.
        let forwarded = CalculationToken::from_harness(metadata("forwarded"), dependencies)
            .emit_shared(tracked.shared());
        assert_eq!(forwarded.id(), &DerivedId::new("forwarded"));
        assert_eq!(
            forwarded.provenance().inputs,
            vec![DerivedId::new("source")]
        );
        assert!(Arc::ptr_eq(&forwarded.shared(), &source.shared()));
    }

    /// Identifiers are copied once per sample, axis, unit and candidate, so
    /// they hold an `Arc<str>` rather than a `String`. Nothing else observes
    /// the difference — every copy still compares and prints equal — so only
    /// pointer identity catches a regression that reintroduces the
    /// per-copy allocation.
    #[test]
    fn copying_an_identifier_shares_one_allocation() {
        let id = DerivedId::new("source");
        let copied = id.clone();
        assert_eq!(id, copied);
        assert!(std::ptr::eq(id.as_str(), copied.as_str()));
        assert!(Arc::ptr_eq(&id.as_shared(), &copied.as_shared()));

        // The copies an emission makes travel the same way.
        let emitted = CalculationToken::from_harness(
            EmitMetadata::new(
                id.clone(),
                PluginId::new("test.plugin"),
                Version::new(0, 1, 0),
                serde_json::json!({}),
                Timestamp::new("2026-01-01T00:00:00Z"),
            ),
            DependencyCollector::default(),
        )
        .emit(1);
        assert!(std::ptr::eq(emitted.id().as_str(), id.as_str()));
        assert!(std::ptr::eq(
            emitted.provenance().algorithm.as_ref(),
            emitted.provenance().producer.as_str()
        ));
    }

    /// A run configures a plugin's parameters once, and every emission that
    /// plugin makes under them records the same tree — once per source,
    /// candidate, or comparator in the loops a stage drives. So `new` takes a
    /// shared handle and keeps it rather than copying what it is given. Only
    /// pointer identity catches a regression: a copied tree still compares,
    /// hashes and serializes identically.
    #[test]
    fn emitting_shares_the_configured_parameter_tree_rather_than_copying_it() {
        let configured = SharedParams::new(serde_json::json!({"threshold": 0.5}));
        let first = CalculationToken::from_harness(
            EmitMetadata::new(
                DerivedId::new("first"),
                PluginId::new("sensor.test"),
                Version::new(0, 1, 0),
                SharedParams::clone(&configured),
                Timestamp::new("2026-09-17T00:00:00Z"),
            ),
            DependencyCollector::default(),
        )
        .emit(1);
        let second = CalculationToken::from_harness(
            EmitMetadata::new(
                DerivedId::new("second"),
                PluginId::new("sensor.test"),
                Version::new(0, 1, 0),
                SharedParams::clone(&configured),
                Timestamp::new("2026-09-17T00:00:00Z"),
            ),
            DependencyCollector::default(),
        )
        .emit(2);
        assert!(Arc::ptr_eq(&first.provenance().params, &configured));
        assert!(Arc::ptr_eq(
            &first.provenance().params,
            &second.provenance().params
        ));
        // The hash still describes exactly the tree that is recorded, which is
        // the invariant `new` derives it for.
        assert_eq!(
            first.provenance().params_hash,
            hash_params(&first.provenance().params)
        );

        // A caller that built a tree for one emission alone hands it over
        // instead, and the record owns that same allocation.
        let owned = CalculationToken::from_harness(
            EmitMetadata::new(
                DerivedId::new("owned"),
                PluginId::new("sensor.test"),
                Version::new(0, 1, 0),
                serde_json::json!({"threshold": 0.5}),
                Timestamp::new("2026-09-17T00:00:00Z"),
            ),
            DependencyCollector::default(),
        )
        .emit(3);
        assert_eq!(owned.provenance().params, configured);
        assert!(!Arc::ptr_eq(&owned.provenance().params, &configured));
    }

    /// Provenance is shared for the same reason the payload is: it carries the
    /// emitting call's whole parameter tree, and a consumer that persists or
    /// retains it should not duplicate it.
    #[test]
    fn provenance_is_shared_across_clones_of_a_derived_value() {
        let source =
            CalculationToken::from_harness(metadata("source"), DependencyCollector::default())
                .emit(1);
        let copied = source.clone();
        assert!(Arc::ptr_eq(
            &source.shared_provenance(),
            &copied.shared_provenance()
        ));
        assert!(std::ptr::eq(
            source.provenance(),
            &*source.shared_provenance()
        ));
    }

    /// A payload that is neither `Clone` nor `Sized` still travels the whole
    /// path, because nothing on it copies the payload.
    #[test]
    fn an_unsized_non_clone_payload_can_be_emitted_tracked_and_cloned() {
        let bytes: Arc<[u8]> = Arc::from(vec![1_u8, 2, 3]);
        let source =
            InferenceToken::from_harness(metadata("source"), DependencyCollector::default())
                .emit_shared(bytes);
        let tracked = Tracked::from(&source);
        let dependencies = DependencyCollector::default();
        let copied_tracked = tracked.clone();
        assert_eq!(dependencies.read(&copied_tracked), &[1, 2, 3]);
        assert!(Arc::ptr_eq(&copied_tracked.shared(), &source.shared()));
        let copied_source = source.clone();
        assert_eq!(copied_source.value(), &[1, 2, 3]);
        assert!(Arc::ptr_eq(&copied_source.into_shared(), &tracked.shared()));
        assert_eq!(
            CalculationToken::from_harness(metadata("result"), dependencies)
                .emit(())
                .provenance()
                .inputs,
            vec![source.id().clone()]
        );
    }

    #[test]
    fn tracked_values_retain_their_in_memory_operation() {
        let interpreted = InterpretationToken::from_harness(
            metadata("interpretation"),
            DependencyCollector::default(),
        )
        .emit(1);
        assert_eq!(
            Tracked::from(&interpreted).operation(),
            Some(Operation::Interpreted)
        );
        assert_eq!(
            Tracked::from_derived(&interpreted, 2).operation(),
            Some(Operation::Interpreted)
        );
        assert_eq!(
            Tracked::from_recorded(DerivedId::new("recorded"), 3).operation(),
            None
        );
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

    /// A panic elsewhere must not turn provenance collection into a second
    /// panic: the inputs recorded before the poisoning are still readable.
    #[test]
    fn a_poisoned_collector_still_reports_what_it_recorded() {
        let collector = DependencyCollector::default();
        collector.read(&Tracked::from_recorded(DerivedId::new("before"), 1));

        let poisoned = collector.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.0.lock().expect("lock is not yet poisoned");
            panic!("poison the collector");
        })
        .join();
        assert!(collector.0.is_poisoned());

        collector.read(&Tracked::from_recorded(DerivedId::new("after"), 2));
        assert_eq!(
            collector.snapshot(),
            vec![DerivedId::new("after"), DerivedId::new("before")]
        );
        assert_eq!(collector.take().len(), 2);
    }

    #[test]
    fn dependency_ids_are_sorted_and_deduplicated() {
        let a = Tracked::from_recorded(DerivedId::new("a"), 1);
        let b = Tracked::from_recorded(DerivedId::new("b"), 2);
        let collector = DependencyCollector::default();
        collector.read(&b);
        collector.read(&a);
        collector.read(&b);
        assert_eq!(
            collector.snapshot(),
            vec![DerivedId::new("a"), DerivedId::new("b")]
        );
    }

    #[test]
    fn membership_and_set_views_match_the_snapshot() {
        let collector = DependencyCollector::default();
        collector.read(&Tracked::from_recorded(DerivedId::new("b"), 2));
        collector.read(&Tracked::from_recorded(DerivedId::new("a"), 1));
        assert!(collector.contains(&DerivedId::new("a")));
        assert!(!collector.contains(&DerivedId::new("c")));
        assert_eq!(
            collector.to_set().into_iter().collect::<Vec<_>>(),
            collector.snapshot()
        );
    }
}
