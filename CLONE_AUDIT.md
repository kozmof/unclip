# Clone audit

Baseline: commit `7d4fb48`. Scope: tracked Rust files throughout `crates/`, including integration tests and inline test modules.

The previous passes moved derived values, tracked inputs, provenance, inference products and every identifier behind an `Arc`, and gave the nine comparator payloads borrowed write-side twins. What they left owned inline was the thing those payloads *report*: a measured value. This pass shares it, and shares the one tree a provenance record still copied — the plugin parameters an emission runs under.

`MeasurementValue` now holds every payload larger than a number behind an `Arc`, so copying a `Reading` copies nothing of what it says. That was the largest remaining duplication by volume and the one the previous pass explicitly declined: it judged a borrowed twin wrong for a value that is emitted and retained as evidence, which was correct — but a *shared* payload is neither a copy nor a borrow, and it is what those records needed. A comparison over several comparators used to keep one full copy of both compared values per comparator; it now keeps one, aliased by all of them.

`EmitMetadata::new` took `&serde_json::Value` and stored `Arc::new(params.clone())`, so the first copy into the provenance `Arc` the previous pass introduced happened once per token — once per plugin per source, candidate, or comparator in the loops a stage drives. Parameters are now `SharedParams` end to end, from the engine profile that parsed them to the row that records them.

Three types also serialized by cloning themselves, through serde's `into` container attribute, which writes `Into::into(self.clone())`. A pairwise matrix copied its whole cell grid on every write; a regime partition copied its starts and, through its inner sequence's own conversion, every coordinate twice. All three now serialize by borrowing, for identical bytes.

## Counting method

Counts are occurrences, not matching lines: a line can contain several calls. Counting all tracked Rust files gives 1335 before and 1312 after. Test counts include files under `tests/` and source text after the first `#[cfg(test)]`; production counts use the preceding source text.

The call-site count is again not the result, and this pass is the clearest case of that so far: it moved by 23 while changing what several hundred of the remaining calls cost. A `Reading` is copied at every hop between a sensor and a row, and each of those copies used to duplicate a matrix, a graph, a ranked state, or an arbitrary structured tree. Each is now a refcount bump, at the same call site, under the same name.

Where the count did move, it moved for two reasons. `unclip-measure`'s production count fell by 6 and its test count rose by 8: `ExpectedIndependentBehavior::reading` stopped copying nine payloads and the sharing guarantees gained four regression tests. `unclip-engine`'s test count fell by 23 because fixtures that pulled a payload out of a reading with `serde_json::from_value(value.clone())` now deserialize from the shared handle instead.

| Crate | Production before | Production after | Test calls after |
| --- | ---: | ---: | ---: |
| unclip-cli | 108 | 108 | 28 |
| unclip-core | 7 | 7 | 5 |
| unclip-domain | 0 | 0 | 3 |
| unclip-engine | 228 | 228 | 455 |
| unclip-epistemic | 18 | 17 | 9 |
| unclip-infer | 13 | 13 | 11 |
| unclip-interpret | 1 | 1 | 3 |
| unclip-io | 5 | 4 | 8 |
| unclip-match | 1 | 1 | 0 |
| unclip-measure | 78 | 72 | 49 |
| unclip-plugin | 18 | 18 | 2 |
| unclip-record | 1 | 1 | 0 |
| unclip-sample | 0 | 0 | 12 |
| unclip-sensors | 62 | 62 | 27 |
| unclip-store | 70 | 70 | 98 |
| **Total** | **610** | **602** | **710** |

## What changed

### A measured value shares its payload

Nine of `MeasurementValue`'s ten variants hold their payload behind an `Arc`; `Scalar(f64)` needs nothing. `Reading::NotApplicable` holds an `Arc<str>` for the same reason, so all four arms of a reading are cheap.

- The variants are shared individually rather than the enum being wrapped as a whole, so a pattern still reaches the payload: matching the `Graph` arm binds the graph, now as a handle that derefs to it. Nested patterns such as `Reading::Value { value: MeasurementValue::PairwiseMatrix(a) }` read exactly as before.
- Serde's `rc` feature is already enabled workspace-wide, so the wire form is unchanged — a shared payload serializes as the bare value it holds. No stored row moves.
- `ExpectedIndependentBehavior` and `IndependentMatrix::Dense` hold their declared payloads in the same shapes, so `reading()` hands out the expectation rather than copying it. An expectation is read once per selected comparator.
- `Applicability::NotApplicable` carries an `Arc<str>`, so a sensor's planning reason becomes the recorded `Reading::NotApplicable` without being copied. Every caller already built one from a literal.

What this is worth is clearest in `compare_product_with_independence`, which emits a baseline measurement per expectation and retains a comparison entry per selected comparator. Each entry reports both whole readings and the baseline reports one, so an expectation over *n* comparators used to materialise 2*n* + 1 copies of values that are arbitrarily large. They now all point at one allocation, which a test asserts by pointer identity through the whole path: the expectation's declared tree, the baseline measurement's reading, and both readings each entry reports.

`PairwiseMatrix` holds its validated data behind an `Arc` too. It is a newtype over a private struct, so this cost no call sites and made copying a matrix a refcount bump everywhere one travels — including inside `IndependentMatrix::Pairwise` and the dense matrices a spectral or community calculation reads.

### Run parameters are shared rather than copied per emission

`unclip-epistemic` names the two shapes: `SharedParams` for one plugin's tree and `PluginParams` for a run's map of them. `EmitMetadata::new` takes `impl Into<SharedParams>` and keeps what it is given, so a caller holding the run's configuration hands over a handle and a caller that built a tree for one emission alone moves it in. Neither copies.

The type runs the length of the path a parameter tree travels, so there is nowhere left for a copy to hide:

- `PluginConfig::params` is shared from the moment an engine profile is parsed, and `ParsedEngineProfile::params` and the CLI's `resolved_profile` carry that same handle rather than rebuilding it.
- `InferenceRun`, `MeasurementRun` and `InterpretationRun` hold a `PluginParams`, as do `run_record` and the engine's internal stage helpers.
- All eight plugin contexts hold `&SharedParams`. `params()` still borrows a `&Params` through the handle, so every existing reader and schema check is unchanged; the new `shared_params()` is how a plugin records what it ran under. It is the only way a plugin outside this workspace can do so without copying, which is why it is on all eight rather than only the three the first-party stages call it from.
- `SensorRunRecord::params` is shared with the configuration the sensor ran under, so recording one run per sensor no longer copies the tree per record.
- `derive_empirical` builds one shared tree for the whole derivation. It emits one structure per input measurement and each used to copy the method parameters; the five cross-domain sensors hoist their configured parameters the same way.

`params_hash` is still derived from `params` inside `new` rather than accepted separately, and the invariant is now stronger: the hash is computed from the exact handle that is stored, so a recorded hash cannot describe a different tree.

### Three types stopped serializing by cloning themselves

`#[serde(into = "T")]` generates `Into::<T>::into(Clone::clone(self)).serialize(..)`. Every write was a deep copy, for bytes a borrow produces identically.

| Type | What a write used to copy |
| --- | --- |
| `PairwiseMatrix` | the whole *n*×*n* cell grid and its unit axis |
| `ObservationSequence` | one coordinate per observation |
| `RegimePartition` | its starts, and every coordinate again through the inner sequence |

Each now has a hand-written `Serialize` that delegates to the borrowed inner data, and keeps `try_from` for deserialization so validation is unchanged. `ObservationSequence` and `RegimePartition` also hold that data behind an `Arc`, so copying one — which a cross-product transfer payload does per side — no longer copies per observation.

## Why clones remain

| Remaining use | Ownership reason |
| --- | --- |
| Identifiers, measured payloads, matrices, sequences, partitions, plugin parameters, database connections, plugin `Arc` handles, dependency collectors, derived and tracked values | Cloning shares an existing allocation. Nothing is duplicated. |
| `to_string()` at the SQLite boundary | SeaORM's `ActiveModel` takes owned values. This is where an identifier's text is finally copied, once per column written. |
| `serde_json::to_value` into a run record | An `EngineRunRecord` is one owned JSON document, so embedding a plugin's parameters in it copies them once. The lookup that feeds it no longer copies, so one copy is now the whole cost. |
| `MeasurementContext` | One production call site copies a context, in the baseline a typed independence comparison emits. The map holds a handful of scalars per measurement, and sharing it would mean reaching through `Arc::make_mut` at the dozen places that build and amend one — the trade this audit's own priorities argue against. |
| Assessments inside emitted values | `ConstraintAssessment`, `ParetoAssessment` and `TransferAssessment` own the readings they report, which is now free: the reading shares its payload. What they still own outright is their own structure. |
| `semver::Version` | An ordinary `major.minor.patch` carries no prerelease or build metadata, so cloning it allocates nothing. |
| Matrix cells | `MatrixCell` is two numbers. Copying one is a stack move. |
| Counterfactual domain snapshots | The proposed domain is independently mutable while the baseline must remain unchanged. |
| Stored provenance, replay snapshots, and evidence records | These are independently owned historical records. |
| A key stored both in a map and its owned record | The current data structures deliberately own both fields. |
| Symmetric matrix cells | Both output positions own their cell; this is not a copy of the matrix. |
| Test inputs reused or mutated in multiple cases | Separate fixtures isolate mutations and preserve the original for assertions. |

Every payload that a run carries from a sensor to a row is now shared: the value measured, the identifier naming it, the provenance recording it, and the parameters it ran under. What is left is either a refcount bump, a value that is genuinely owned twice, or the single owning copy a row write or a run document has to make. The practical priority for any further pass is unchanged, and the remaining candidates are small by it: `MeasurementContext`, and the owned JSON a record assembles. No unsafe code or lifetime extension is used.

## Unrelated finding

Carried forward unchanged from the previous passes, because it is a correctness question rather than a copying one:

- `leveling::discovery::calculate` restores selected empirical structures as `Inferred` while `leveling::discovery::discover` tracks the same structures as `Calculated`. Only calculation writes them. The mislabel is inert today because candidate generation has no calculated-evidence gate, but `Engine::interpret` does, and it would reject a structure restored from a discovery snapshot.
- `Engine::run_counterfactual_experiment` restores a split's held-out observations unlabeled (`None`) while `Engine::measure_held_out_baseline` restores the same entries as `Inferred`.

## Validation

Validation passed: workspace Clippy over all targets with warnings and redundant clones denied, all 604 workspace tests, 7 doctests, documentation with warnings denied, release build, formatting, and workspace packaging through `.github/scripts/package-workspace.sh`.

New regression tests. Every guarantee this pass establishes is invisible to equality, hashing and serialization, so each is pinned by pointer identity:

- Copying a `Reading` shares the payload of all nine non-scalar kinds and the reason a `NotApplicable` arm carries, asserted alongside the equality and byte-for-byte checks that a regression would still satisfy.
- A typed independence comparison reports one allocation, not one per comparator: the expectation's declared tree, the baseline measurement's reading, and both readings every retained entry reports are asserted to be the same tree the composed product measurement holds.
- `ExpectedIndependentBehavior::reading` hands out the declared graph, ranked state, or reason rather than a copy.
- Copying a `PairwiseMatrix`, an `ObservationSequence` or a `RegimePartition` shares one allocation, and building a partition shares the sequence it was given. Each assertion sits beside a round trip that pins the unchanged wire form, since the hand-written `Serialize` impls are what keep it.
- Two emissions under one configured parameter tree record that tree itself, and a caller that built a tree for one emission still owns a distinct one. The test also asserts that the recorded hash still describes exactly the recorded tree.
- A plugin context hands out the run's parameters for reading and for recording without copying them, asserted on the provenance an emission through that context carries.
