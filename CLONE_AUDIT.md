# Clone audit

Baseline: commit `5d2bd01`. Scope: tracked Rust files throughout `crates/`, including integration tests and inline test modules.

Not all remaining clones were needed. This pass removes 151 explicit `.clone()` calls: 138 in production code and 13 in tests. It also removes implicit deep copies made by `Tracked::from(&derived)` and intermediate collections built with `.cloned()`.

## Counting method

Counts are occurrences, not matching lines: a line can contain several calls. Counting all tracked Rust files gives 1653 before and 1502 after. Test counts include files under `tests/` and source text after the first `#[cfg(test)]`; production counts use the preceding source text. This explains why a search with different paths or line counting can produce a different total. These are static call-site counts, not allocation counts or measured memory savings.

| Crate | Production before | Production after | Test calls after |
| --- | ---: | ---: | ---: |
| unclip-cli | 144 | 140 | 28 |
| unclip-core | 7 | 7 | 5 |
| unclip-domain | 0 | 0 | 3 |
| unclip-engine | 402 | 313 | 478 |
| unclip-epistemic | 17 | 17 | 1 |
| unclip-infer | 13 | 13 | 11 |
| unclip-interpret | 3 | 1 | 3 |
| unclip-io | 9 | 5 | 8 |
| unclip-match | 1 | 1 | 0 |
| unclip-measure | 79 | 78 | 41 |
| unclip-plugin | 18 | 18 | 2 |
| unclip-sample | 0 | 0 | 12 |
| unclip-sensors | 88 | 62 | 27 |
| unclip-store | 136 | 124 | 104 |
| **Total** | **917** | **779** | **723** |

## Avoidable copies removed

- **Sample maps:** canonical correlation, mutual information, and interaction movement now accept borrowed sample iterators. Existing slice APIs remain available. Sensors retain only references to selected samples, with eager dependency reads preserved even on early returns.
- **Provenance reads:** `DependencyCollector::read_derived` reads a derived payload without constructing an owned tracked copy. Recording an already-seen dependency no longer allocates another ID just to discard it. Experiment, revision, comparison, and composition paths use this where they only need a read.
- **Temporary indexes:** product coordinates, alignment indexes, frame positions, validation sets, discovery labels, and comparison membership use references where the inputs outlive the indexes.
- **JSON:** additional parameter/proposal deserialization paths borrow JSON trees; candidate JSON objects are moved out of fresh literals; revision validation compares borrowed properties; ledger hydration scans borrowed evidence values.
- **Owned intermediates:** graph and partition comparisons move their canonicalized inputs into results; sensor summaries move excluded/unassessed lists after serialization; interpretation moves its request structure into the returned result after the request borrow ends.
- **Persistence preparation:** domain persistence moves consumed unit/relation IDs; replay queries iterate selected provenance IDs; ledger string scanning borrows strings until an owned provenance ID is actually needed.
- **Tests:** removed all 13 redundant copies identified by the additional `clippy::redundant_clone` audit. Intentional fixture copies remain.

## Why clones remain

| Remaining use | Ownership reason |
| --- | --- |
| Database connections, plugin `Arc` handles, dependency collectors | Cloning shares an existing allocation or pool. It does not duplicate the database, plugin, or collector contents. |
| Measurement/error/result IDs, versions, readings, units, and relations | Returned records and errors own their data and can outlive the borrowed inputs. |
| Counterfactual domain snapshots | The proposed domain is independently mutable while the baseline must remain unchanged. |
| Stored provenance, replay snapshots, and evidence records | These are independently owned historical records. Sharing or borrowing them would require changing their representation or persistence interfaces. |
| A key stored both in a map and its owned record | The current data structures deliberately own both fields. |
| Symmetric matrix cells | Both output positions own their cell; this is not a clone of the entire matrix. |
| Test inputs reused or mutated in multiple cases | Separate fixtures isolate mutations and preserve the original for assertions. |

These reasons explain the remaining patterns; they are not a claim that every remaining clone is inherently unavoidable. In particular, the following ownership boundaries still copy substantial data and are candidates for a larger API redesign:

- `Tracked<T>` owns `T`. Held-out measurement, replay, and comparator APIs that require `Tracked<Measurement>` or `Tracked<Observation>` still need owned payloads when callers retain the originals. A borrowed view or shared payload representation would affect plugin context and public API types.
- CLI persistence sometimes retains calculated outputs for display while repositories accept them by value. Borrowing repository inputs, or separating display records before consuming outputs, could remove further copies.
- Serialized comparison/result types own their embedded readings and evidence. Separate borrowed serialization views could reduce transient copies but would add parallel representations.

The practical priority is large payloads and copies repeated per sample, axis, unit, or candidate, rather than eliminating cheap handle clones merely to reduce the count. No unsafe code or lifetime extension is used.

## Validation

Validation passed: workspace Clippy with warnings and redundant clones denied, all 578 workspace tests, doctests, documentation with warnings denied, formatting, and the sensor dependency policy. New regression tests verify that provenance reads accept a non-Clone payload and preserve deduplicated dependencies, and that an empty-frame sensor result still records every selected sample.
