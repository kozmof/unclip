# Clone audit

Baseline: commit `71d6fda`. Scope: tracked Rust files throughout `crates/`, including integration tests and inline test modules.

The previous pass moved four workspace-wide types — derived values, tracked inputs, provenance, and inference products — behind an `Arc`, and left two boundaries named and deliberately untaken. This pass takes both of them.

Identifiers no longer own a `String`. `string_id!` declares each one as a newtype over `Arc<str>` with a private field, so copying any identifier in the workspace is a refcount bump. That was the largest remaining source of duplication by volume: an identifier is copied once per sample, axis, unit and candidate on every path, and each copy used to allocate. The eight remaining comparator payloads that exist only to be serialized now have borrowed write-side twins, the shape the scalar comparator already used, so building one copies nothing out of the measurements it reports.

## Counting method

Counts are occurrences, not matching lines: a line can contain several calls. Counting all tracked Rust files gives 1445 before and 1335 after. Test counts include files under `tests/` and source text after the first `#[cfg(test)]`; production counts use the preceding source text.

As in the previous pass, the call-site count is not the result, and the two largest movements in the table illustrate opposite things. `unclip-store`'s production count fell by 75 almost entirely because `Set(id.0.clone())` became `Set(id.to_string())` — the same allocation under a name Clippy does not count, with no change in what the boundary copies. The nine comparators went from 59 production clones to 17, and there the count and the cost moved together: what each removed clone used to copy was a whole reading, graph or structured tree.

| Crate | Production before | Production after | Test calls after |
| --- | ---: | ---: | ---: |
| unclip-cli | 111 | 108 | 28 |
| unclip-core | 7 | 7 | 5 |
| unclip-domain | 0 | 0 | 3 |
| unclip-engine | 282 | 228 | 478 |
| unclip-epistemic | 18 | 18 | 9 |
| unclip-infer | 13 | 13 | 11 |
| unclip-interpret | 1 | 1 | 3 |
| unclip-io | 5 | 5 | 8 |
| unclip-match | 1 | 1 | 0 |
| unclip-measure | 78 | 78 | 41 |
| unclip-plugin | 18 | 18 | 2 |
| unclip-record | 1 | 1 | 0 |
| unclip-sample | 0 | 0 | 12 |
| unclip-sensors | 62 | 62 | 27 |
| unclip-store | 145 | 70 | 98 |
| **Total** | **742** | **610** | **725** |

## What changed

### Identifiers hold a shared string

`string_id!` declares `pub struct $name(Arc<str>)`. Every identifier in the workspace goes through that macro — `DerivedId`, `PluginId`, `Timestamp`, `ParameterHash`, `DomainId`, `UnitId`, `RelationId`, `FrameId`, `ObservationId`, the product coordinates, and the version newtypes — so one change covers all nineteen.

- The field is private. Reads go through `as_str`, `AsRef<str>`, `Deref<Target = str>` or `Display`; `as_shared` hands out the `Arc<str>` itself. Making it private is what forced every one of the several hundred former `.0` reads to be looked at rather than silently recompiled.
- `new` takes `impl Into<Self>`, and `From` is implemented for `&str`, `String`, `&String`, `Cow<str>`, `Arc<str>` and `&Self`, so construction reads the same as before.
- `Borrow<str>` lets a map keyed by `String` be probed with an identifier, and `Deref` lets `id.trim()` and `id.is_empty()` read as they did when the field was public.
- `Hash`, `Ord` and `PartialEq` still delegate to the string, and `#[serde(transparent)]` still writes a bare string, so no stored row, hash or sort order moves.

`Provenance.algorithm` became an `Arc<str>` for the same reason. It is written once per emission and defaults to the producing plugin's id, which it now shares with the `producer` field instead of copying.

The one place this costs something is the SQLite boundary, which the previous pass predicted: SeaORM's `ActiveModel` takes owned `String`s, so a row write now calls `to_string()` where it used to move or clone a `String`. The allocation count there is unchanged — the write always had to own its columns — and everything upstream of it stopped allocating.

### Comparator payloads borrow what they serialize

Each comparator builds a payload, turns it into JSON, and drops it. Nine payloads are shaped that way; the scalar one already had a borrowed write-side twin, and the other eight now do too, each pinned to its owning shape by a round-trip test over every arm:

| Payload | What it stopped copying |
| --- | --- |
| `StructuredIdentityComparison` | both measured values, which are arbitrary structured trees |
| `RankingComparison` | both ranked states, which own a tier vector per unit |
| `GraphComparison` | both graphs and all four node/edge difference lists |
| `EventComparison` | every change point in the matched, removed and added lists |
| `DistributionComparison` | one `String` per reported category |
| `MatrixComparison` | the shared unit axis |
| `SpectralComparison`, `PartitionComparison` and every payload above | the two whole readings reported when a comparison is unavailable |

The `Unavailable` arms are the case the previous pass judged too small to be worth it. That judgement was right for the readings those arms carry, but wrong about the `Value` arms of the structured, ranking, graph and event comparators, which report measured payloads verbatim and copied them in full.

`CompareCtx::before` and `after` now return a reference tied to the context's own lifetime rather than to the borrow of the context, which is what lets a payload outlive the call that builds it.

### Supporting changes

- `PluginSelection::any` takes `impl Into<PluginId>` rather than `impl Into<String>`, so a caller that already holds an id hands it over instead of rebuilding it.
- The partition, graph and event comparators hoist their parsed values above the payload so it can borrow them; the parse itself is unchanged.

## Why clones remain

| Remaining use | Ownership reason |
| --- | --- |
| Identifiers, database connections, plugin `Arc` handles, dependency collectors, derived and tracked values | Cloning shares an existing allocation. Nothing is duplicated. |
| `to_string()` at the SQLite boundary | SeaORM's `ActiveModel` takes owned values. This is where an identifier's text is finally copied, once per column written. |
| Readings and assessments inside emitted values | `ConstraintAssessment`, `ParetoAssessment` and `TransferAssessment` are emitted and retained as evidence, not serialized and dropped, so they own what they report. A borrowed twin would be wrong here, not merely unnecessary. |
| `semver::Version` | An ordinary `major.minor.patch` carries no prerelease or build metadata, so cloning it allocates nothing. |
| Matrix cells | `MatrixCell` is two numbers. Copying one is a stack move. |
| Counterfactual domain snapshots | The proposed domain is independently mutable while the baseline must remain unchanged. |
| Stored provenance, replay snapshots, and evidence records | These are independently owned historical records. |
| A key stored both in a map and its owned record | The current data structures deliberately own both fields. |
| Symmetric matrix cells | Both output positions own their cell; this is not a copy of the matrix. |
| Test inputs reused or mutated in multiple cases | Separate fixtures isolate mutations and preserve the original for assertions. |

Both boundaries the previous pass deferred are now closed. What is left is either a refcount bump, a value that is genuinely owned twice, or the single owning copy a row write has to make. The practical priority for any further pass stays the same: large payloads and copies repeated per sample, axis, unit, or candidate, rather than the call-site count. No unsafe code or lifetime extension is used.

## Unrelated finding

Carried forward unchanged from the previous pass, because it is a correctness question rather than a copying one:

- `leveling::discovery::calculate` restores selected empirical structures as `Inferred` while `leveling::discovery::discover` tracks the same structures as `Calculated`. Only calculation writes them. The mislabel is inert today because candidate generation has no calculated-evidence gate, but `Engine::interpret` does, and it would reject a structure restored from a discovery snapshot.
- `Engine::run_counterfactual_experiment` restores a split's held-out observations unlabeled (`None`) while `Engine::measure_held_out_baseline` restores the same entries as `Inferred`.

## Validation

Validation passed: workspace Clippy over all targets with warnings and redundant clones denied, all 592 workspace tests, 7 doctests, documentation with warnings denied, release build, formatting, and workspace packaging.

Packaging needed a fix of its own before it could say anything. `cargo package --locked --workspace` republishes every internal crate into a throwaway registry under one unchanging version, and Cargo caches a registry source and its artifacts as immutable, so verification was compiling this commit's crates against whatever the internal API was the last time the step ran. Any commit that changes an internal API fails there, which is how this one surfaced it — and the same staleness can let a commit pass that would not build on a clean machine. `.github/scripts/package-workspace.sh` drops the two caches first; CI and the README both call it now.

New regression tests:

- Copying an identifier shares one allocation, asserted by pointer identity on both the identifier itself and on the id an emission carries into its provenance, and on the algorithm an emission shares with its producer. A regression here is otherwise silent, since every copy still compares, hashes and prints equal.
- The `string_id!` doctest asserts the same pointer identity for a freshly declared identifier, so the guarantee is part of the documented contract.
- One round-trip test per comparator payload — structured, ranking, graph, event, distribution, matrix, partition and spectral — pinning the borrowed write-side shape to the owning shape it must deserialize into, over every arm. These join the two the previous pass added for `StoredContextRef` and `ScalarDifferenceRef`.
