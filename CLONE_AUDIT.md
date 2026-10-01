# Clone audit

Baseline: commit `ff9d157`. Scope: tracked Rust files throughout `crates/`, including integration tests and inline test modules.

The previous pass removed redundant call sites. This one changes where values live. Four types that every stage handles — derived values, tracked inputs, provenance, and the inference products a run infers — now hold their payload behind an `Arc` instead of owning it inline. Copying them is a refcount bump, so the copies that scaled with the number of observations, measurements, and emissions are gone rather than merely fewer.

## Counting method

Counts are occurrences, not matching lines: a line can contain several calls. Counting all tracked Rust files gives 1502 before and 1445 after. Test counts include files under `tests/` and source text after the first `#[cfg(test)]`; production counts use the preceding source text.

**The call-site count is the wrong measure for this pass, and it moved the wrong way in places.** A surviving `.clone()` on an `Arc`-backed value copies a pointer and an integer; writing one provenance row now clones eight short column strings where it used to accept one deep copy of the whole record, including its parameter tree. `unclip-store`'s production count rose by 21 for exactly that reason — borrowed row writes trade one large copy for several tiny ones — and it copies strictly less than before. The table is here for continuity with the previous pass, not as the result.

| Crate | Production before | Production after | Test calls after |
| --- | ---: | ---: | ---: |
| unclip-cli | 140 | 111 | 28 |
| unclip-core | 7 | 7 | 5 |
| unclip-domain | 0 | 0 | 3 |
| unclip-engine | 313 | 282 | 458 |
| unclip-epistemic | 17 | 18 | 7 |
| unclip-infer | 13 | 13 | 11 |
| unclip-interpret | 1 | 1 | 3 |
| unclip-io | 5 | 5 | 8 |
| unclip-match | 1 | 1 | 0 |
| unclip-measure | 78 | 78 | 41 |
| unclip-plugin | 18 | 18 | 2 |
| unclip-record | 0 | 1 | 0 |
| unclip-sample | 0 | 0 | 12 |
| unclip-sensors | 62 | 62 | 27 |
| unclip-store | 124 | 145 | 98 |
| **Total** | **779** | **742** | **703** |

## What changed

### Shared payloads

`Derived<T, O>` and `Tracked<T>` hold `Arc<T>`. The consequences reach every stage:

- `Tracked::from(&derived)` — the most common hop in the workspace, at 107 call sites — no longer copies the payload. Neither does cloning a `Derived` or a `Tracked`.
- Both types are now cloneable, comparable and debuggable independently of their payload, because the hand-written impls are not bounded on `T`. Both accept `T: ?Sized`, so an unsized payload travels the whole path.
- `Derived::into_value` copies only when the value is still shared, which for a value emitted and immediately consumed it is not.
- `EmitToken::emit_shared` emits a payload that is already shared, so a stage handing one value to both its result and a downstream input allocates once.
- `DependencyCollector::read_shared` and `read_derived_shared` record an input and hand back a handle, for the callers that must keep the value past the borrow.

`Provenance` is shared by the same mechanism. One token emits several values from one analysis and they all record the same parameters, so `Provenance.params` is an `Arc` too: the parameter tree is copied once per token instead of once per emission, and `Derived::shared_provenance` lets a consumer keep or persist a provenance record without duplicating it.

### Shared payloads in the records that carry them

Each of these held a private copy of something its producer still owned:

| Record | Shared with |
| --- | --- |
| `InferenceOutput` products | the tracked observations, alignments and rankings the engine derives from them |
| `RecordedInference.value` | replay, split and transfer evidence restored from it, via `tracked` / `into_tracked` |
| `MeasurementRecord.measurement` | the tracked evidence a caller reads from it, via `tracked` / `into_tracked` |
| `StoredProvenance.provenance` | the emitted value it was built from, via `StoredProvenance::of` |
| `CounterfactualSnapshot.domain` | the tracked counterfactual measured against it |
| `ProfileMeasurement.measurement` | the derived measurement a composition profile indexes |
| `ProfileDelta.delta`, `IndependenceComparisonEntry.delta` | the calculated deltas returned beside the profile |
| `CounterfactualEvidence`'s delta profile, null readings, constraints and Pareto assessment | the calculated values returned beside it on `CounterfactualExperiment` |

The inference one is the largest: the engine keeps an emitted output for provenance *and* tracks every product inside it as separate engine evidence, so each observation used to be copied once per run, scaling with the number of observations.

### Borrowed inputs and borrowed serialization views

- `ObservationRepository::insert_observation`, `insert_alignment` and `insert_ranking` take their value by reference. Writing a row copies only the short column values it has to own, and the inference stage keeps every product it stores.
- Writing a measurement row borrows the measurement's reading and context instead of moving out of it. The destructure is still exhaustive, so a new field on `Measurement` fails to compile until it is stored or explicitly ignored.
- `StoredContextRef` and `ScalarDifferenceRef` are write-side twins of the owning shapes that readers deserialize into. Each is pinned to its owning shape by a round-trip test, because the two are one wire format and a drift between them would silently corrupt every row written afterwards.
- The candidate-application revision path moves the proposed domain out of the application it just produced rather than copying it; the CLI experiment path shares one domain snapshot and frame between its tracked inputs and the versions it later reads off them.

## Why clones remain

| Remaining use | Ownership reason |
| --- | --- |
| Database connections, plugin `Arc` handles, dependency collectors, derived and tracked values | Cloning shares an existing allocation. Nothing is duplicated. |
| Identifiers (`DerivedId`, `PluginId`, `UnitId`, …) | Each is a `String` newtype, so every copy allocates. They are copied per sample, axis, unit and candidate, and this is now the largest remaining category. See below. |
| Short column strings at the SQLite boundary | SeaORM's `ActiveModel` takes owned values. Copying a handful of short strings per row is what lets the record itself be borrowed. |
| Counterfactual domain snapshots | The proposed domain is independently mutable while the baseline must remain unchanged. |
| Stored provenance, replay snapshots, and evidence records | These are independently owned historical records. |
| A key stored both in a map and its owned record | The current data structures deliberately own both fields. |
| Symmetric matrix cells | Both output positions own their cell; this is not a copy of the matrix. |
| Test inputs reused or mutated in multiple cases | Separate fixtures isolate mutations and preserve the original for assertions. |

Two boundaries remain as candidates for a further pass, both deliberately not taken here:

- **Identifiers allocate on every copy.** `string_id!` declares each identifier as `pub struct $name(pub String)`. Backing it with `Arc<str>` would turn every per-sample identifier copy into a refcount bump, which is the single biggest remaining win by volume. It is also the most invasive change available: the `.0` field is public and read as a `String` at hundreds of sites, most of them the SQLite boundary, which needs owned `String`s and would have to allocate there instead. That trade is worth measuring before it is worth making.
- **Comparator payloads other than the scalar one.** `GraphComparison`, the distribution comparators and the transfer-constraint payload build an owned value purely to serialize it, exactly as the scalar comparator did. Each would take the same borrowed write-side twin. They were left alone because in every one of them the arm that copies a reading is reached only when at least one side is *not* a measured value, so the readings it copies are small — the pattern is right but the payoff is not yet there.

The practical priority remains large payloads and copies repeated per sample, axis, unit, or candidate, rather than eliminating cheap handle clones to reduce the count. No unsafe code or lifetime extension is used.

## Unrelated finding

Two paths label the same class of value differently, which this pass preserved rather than corrected because it is a correctness question, not a copying one:

- `leveling::discovery::calculate` restores selected empirical structures as `Inferred` while `leveling::discovery::discover` tracks the same structures as `Calculated`. Only calculation writes them. The mislabel is inert today because candidate generation has no calculated-evidence gate, but `Engine::interpret` does, and it would reject a structure restored from a discovery snapshot.
- `Engine::run_counterfactual_experiment` restores a split's held-out observations unlabeled (`None`) while `Engine::measure_held_out_baseline` restores the same entries as `Inferred`.

## Validation

Validation passed: workspace Clippy with warnings and redundant clones denied, all 583 workspace tests, 7 doctests, documentation with warnings denied, release build, formatting, and the sensor dependency policy.

New regression tests:

- Tracking, cloning and re-emitting a derived value alias one payload allocation, asserted by pointer identity — a regression here is otherwise silent, since every value still compares equal.
- Provenance is shared across clones of a derived value.
- An unsized, non-`Clone` payload can be emitted, tracked, cloned and read.
- `StoredContextRef` writes exactly what `StoredContext` reads, over both sparse and non-sparse readings.
- `ScalarDifferenceRef` writes exactly what `ScalarDifference` reads, over all three arms.
