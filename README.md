# unclip

unclip is a CLI for getting varied output from LLMs by building
the possibility space outside the model. Store ideas as addressable branches,
index and constrain them, then sample structured selections to feed the model.

## Data structure

- branch — an addressable node at a slash-separated path, e.g. `/ikebukuro/station/exit`.
- path — the hierarchical scope a branch lives under.
- o2o — one-to-one indexed values. Each name holds exactly one value per branch.
- o2m — one-to-many indexed values. Each name holds a set of values per branch.
- metadata — a free-form JSON payload for richer content.
- frame — reusable constraints made of named slots.
- packet — a sampled, portable selection you can save or pipe to a model.

Sampling applies hard and soft filters. A required o2o value and a required o2m
value must match for a branch to be eligible. A preferred o2m value only raises
a branch's score. An avoided value excludes it.

## Install

unclip is currently distributed from source. The repository pins its Rust
toolchain in `rust-toolchain.toml`, and Cargo uses it automatically when rustup
is installed.

Build the release binary from a checkout.

```bash
cargo build --release --locked
```

The binary is `target/release/unclip`. Copy it onto your `PATH`, or run it
through Cargo while developing.

```bash
cargo run -p unclip-cli -- --help
```

To install the CLI from a local checkout, use Cargo path installation.

```bash
cargo install --path crates/unclip-cli --locked
```

The database is a single SQLite file. It defaults to `unclip.db` in the current
directory. Point any command at another file with `--db`.

## Quick start

Create the database.

```bash
unclip init
```

Add a branch with its coordinates and qualities.

```bash
unclip add /ikebukuro/station/exit \
  --o2o domain=story \
  --o2o axis=place \
  --o2m density=crowded \
  --o2m topic=transit \
  --title "Ikebukuro Station Exit"
```

Inspect what you have.

```bash
unclip show /ikebukuro/station/exit
unclip ls /ikebukuro/station
unclip tree /ikebukuro
```

Find branches by scope and filters.

```bash
unclip query \
  --under /ikebukuro \
  --o2o domain=story \
  --require-o2m topic=transit
```

Sample a selection packet under constraints.

```bash
unclip sample \
  --under /ikebukuro \
  --o2o domain=story \
  --o2o axis=place \
  --prefer-o2m density=crowded \
  --avoid-o2m topic=cafe
```

Compose a packet with one selection group per frame slot, then save it.

```bash
unclip compose --frame story --under place:/ikebukuro --format yaml > seed.yaml
```

## Commands

Branches and scope.

- `init` — create and migrate the database.
- `add`, `edit` — create a branch or change its fields, o2o, and o2m.
- `rm` — delete a branch, or its whole subtree with `--recursive`.
- `show`, `ls`, `tree` — view a branch, its children, or its subtree.
- `query` — find branches by scope and hard o2o/o2m filters.
- `o2o`, `o2m` — browse the value catalogs or the branches that carry a value.

Frames.

- `import-frames` — load frame definitions from YAML.
- `frames`, `frame` — list frames or show one frame or slot.
- `rm-frame` — delete a frame and its slots.
- `create` — make a skeleton branch from a frame slot.
- `validate` — check a branch or a packet file against a frame.

Sampling and usage.

- `sample` — draw branches into a selection packet.
- `compose` — build a packet with one group per frame slot.
- `replay` — re-run the sampling recorded in a packet file.
- `used`, `stats`, `stale` — review usage history and least-used branches.

Exchange and references.

- `import`, `export` — move branches in and out as YAML, JSON, or JSONL.
- `attach`, `refs` — link external files or URLs to a branch and list them.

Matching.

- `scan` — find archive patterns inside a text file.
- `suggest-o2m` — propose o2m values mentioned in a branch but not yet set.
- `pattern`, `patterns` — manage the user-defined pattern dictionary.

Run `unclip <command> --help` for the full options of any command.

## Output and reproducibility

`unclip level measure obs-1 obs-2 --profile engine.yaml` calculates a profile over
the explicitly selected observations. A single observation still uses the same
command. Batch profiles can select `sensor.trajectories`, `sensor.spearman`,
`sensor.kendall-association`, `sensor.relative-rank-variance`, and
`sensor.mutual-information`. Selection order is recorded; these batch sensors
use stable observation-ID order and make no temporal claims.

Pair-specific sensors use explicit parameters in the engine profile.
`sensor.co-foreground` accepts `left`, `right`, and a positive `foreground_rank`.
`sensor.conditional-mutual-information` and `sensor.partial-correlation` accept
`left`, `right`, and `conditioning_variables: [unit-id]`; each currently supports
one conditioning unit. They report complete-case sample counts and preserve
insufficient evidence separately from measured zero.

Temporal profiles select `sensor.lagged-dependency` (`source`, `target`, positive
`lag`), `sensor.dtw` (`left`, `right`), or `sensor.change-points` (`unit`, positive
`window` and `minimum_shift`). Each requires a `sequence` parameter, for example
`[{observation: obs-1, position: 0}, {observation: obs-2, position: 10}]`, covering
every selected observation exactly once with strictly increasing positions. Lag
and window sizes count sequence steps; timestamps alone do not establish order.
Lagged association is directional evidence without a causal claim. DTW compares
two units over the same complete sequence using unnormalized absolute rank cost.
Missing evidence stays distinct from a measured zero or an empty change-event list.

The engine library exposes `Engine::derive_empirical` for explicitly selected,
tracked matrix measurements from profiles. Choose `EmpiricalMethod::Communities`
or `EmpiricalMethod::Spectral` with explicit thresholds and evidence requirements.
Each source produces its own anonymous structure or an absent-evidence result;
metrics are kept separate. Successful structures carry calculation provenance
and can be stored with `MeasurementRepository::insert_calculated_structure`.
Derive anonymous structures from stored profiles with an explicit YAML or JSON
configuration:

```sh
unclip level derive profile-1 profile-2 --config communities.yaml
unclip level structure '<structure-id>' --format json
unclip level verify '<empirical-run-id>'
```

```yaml
method: communities
threshold: 0.5
minimum_samples: 2
```

For spectral decomposition, use `method: spectral`, `minimum_samples: 2`,
`tolerance: 0.000000000001`, and `max_sweeps: 100`. Each selected profile must
contain matrix measurements; other measurement kinds are skipped. Each matrix
produces an independent result, retaining its source profile and measurement
provenance. Sparse matrices may report `INSUFFICIENT_EVIDENCE`; no structure is
fabricated. Empirical runs snapshot their selected measurements and method,
and verification recalculates both values and provenance against stored results.
Results remain anonymous: this command assigns no semantic labels.

Engine-profile YAML and JSON accept optional `candidate_generators` and
`null_models` lists, using the same `id`, `version`, and object-valued `params`
fields as sensors. Selections are explicit and version-checked; recorded plans
pin resolved versions, parameters, and hashes. Older profiles default these
lists to empty. Built-in generators are `generate.persistent-residual`,
`generate.missing-relation`, `generate.recurring-motif`, and
`generate.pairwise-coupling`, `generate.temporal-coupling`,
`generate.community`, and `generate.latent-axis`;
null models are `null.random-cooccurrence`, `null.ranking-constraints`,
`null.existing-unit`, `null.existing-relation`, `null.weight-change`, and
`null.contextual-cooccurrence`.

The engine library runs candidate generators through `Engine::generate_candidates`
with tracked residual measurements and source observations from one baseline
domain version. Configure `params: {minimum_observations: 2}` (minimum 2).
`generate.persistent-residual` groups unmatched units by exact, case-sensitive
observed label and counts
distinct observations. Repeated units or profiles cannot inflate that count.
Proposals retain observed examples and provenance, without assigning a semantic
label or changing the domain. Sparse evidence produces no proposal.

`generate.missing-relation` uses the same minimum-observation parameter and
tracked inputs, but reads unexplained-relation residuals. It groups exact directed
(source label, relation kind, target label) patterns, retaining relation identities
and uncertainty in the examples. Reversed edges and different relation kinds
remain separate. It emits relation proposals.

`generate.recurring-motif` proposes directed two-edge paths through three distinct
observed units. Both edges must be unexplained residuals in the same observation.
Exact node labels and directed relation kinds define a pattern; matching labels
on disconnected units do not establish connectivity. The same minimum-observation
parameter applies. Each proposal retains the graph shape and both edges’ evidence
and uncertainty. Additional edges do not disqualify a path; this generator does
not enumerate arbitrary larger motifs.

`generate.pairwise-coupling` reads typed pairwise matrices from tracked
measurements. Configure, for example, `params: {metric: spearman, threshold: 0.8,
minimum_samples: 4}`. Supported metrics are `spearman`, `kendall`,
`mutual_information`, and `relative_rank_variance`. Correlation thresholds are
in [-1, 1]; other thresholds are nonnegative. The sample floor is at least 2.
Variance qualifies at or below its threshold; other metrics qualify at or above
it, preserving correlation signs. Only measured off-diagonal cells qualify.
Each source matrix produces separate coupling hypotheses with its own metric,
cell sample count, context, and evidence. Nothing is averaged across profiles,
and these hypotheses make no causal claim.

`generate.temporal-coupling` uses measured `sensor.lagged-dependency` results.
Configure `params: {threshold: 0.8, minimum_samples: 3}`; the signed coefficient
must meet the threshold in [-1, 1] and the sample floor must be at least 2.
Proposals preserve source, target, lag in sequence steps, explicit observation
order, and complete-pair sample count. Missing order or inconsistent measured
evidence is rejected; sparse readings produce no proposal. Each source remains
separate and carries no causal claim. DTW and change-point results are not used
by this generator.

`generate.community` reads tracked community structures with parameters such as
`{metric: spearman, minimum_samples: 2, minimum_members: 2}`. It proposes
composites retaining member sets, the full partition, and unassessed pairs. The
source community calculation must meet the requested sample floor.

`generate.latent-axis` reads tracked spectral structures with parameters such as
`{metric: spearman, minimum_samples: 2, minimum_absolute_eigenvalue: 0.1}`.
Each qualifying axis retains signed eigenvalues and loadings; negative eigenvalues
are not interpreted as explained variance. Both generators keep sources separate
and produce anonymous proposals, without changing the domain or assigning labels.
Axes in degenerate eigenspaces need not have a unique interpretation.

The library evaluates selected null models through `Engine::evaluate_null_models`,
using a tracked candidate and an explicit observation set. Configure
`null.random-cooccurrence` with `{minimum_observations: 2}`. For exact observed-label
relation candidates with distinct endpoint labels, it counts label presence once
per observation and computes the hypergeometric upper-tail overlap probability
conditional on both observed frequencies. It reports expected and observed overlap,
counts, assumptions, and full input provenance. Same-label or unsupported candidates
return `NotApplicable`; small samples return `InsufficientEvidence`.

This null tests endpoint co-presence only, not relation direction or kind. It assumes
exchangeable observations and does not control source, time, genre, extraction bias,
or candidate selection. Results do not automatically accept or reject candidates.
Other null families, held-out split enforcement, and persisted experiment
execution remain pending.

`null.ranking-constraints` uses the same minimum-observation parameter, applied to
untied, comparable endpoint pairs. Supply tracked partial rankings through
`Engine::evaluate_null_models_with_rankings`. For observed-label relation proposals,
each endpoint label must identify a unique observed unit. The null conditions on
endpoint availability and pair tie status, then independently swaps the two labels
within each untied pair. The number of source-before-target observations follows
a binomial distribution with probability one half. Results retain counts of ties,
skipped pairs, per-observation status, and an upper-tail probability. Unknown or
missing ranks are never completed. This bounded null explains endpoint order, not
general rank correlations, relation kind, or temporal dependence; selection and
source biases are not adjusted.

`null.existing-unit` and `null.existing-relation` take empty parameters and a
tracked baseline domain through `NullInputs` and
`Engine::evaluate_null_models_with_inputs`. The first checks atomic observed-label
proposals against atomic domain units with exactly matching labels. The second
checks exact directed observed-relation proposals against existing relations,
matching endpoint labels, direction, and relation kind. Both retain every matching
identity without claiming semantic equivalence or accepting or rejecting a candidate.
The baseline must match the candidate domain version; malformed domain identities
or dangling relation endpoints are errors. A missing baseline is insufficient
evidence, while a valid baseline with no matches yields an explicit empty result.
These checks never mutate the domain.

`null.weight-change` checks the alternative of retaining an existing numeric
property. Configure `{absolute_tolerance: 0}` or an explicit nonnegative tolerance.
It accepts `WeightRevision` candidates with a pattern such as:

```json
{"matching":"numeric_property_revision","target":{"kind":"unit","id":"a"},"property":"weight","proposed_value":0.5}
```

Targets can be units or relations. The property name is explicit; no field is
automatically treated as a weight. The result retains baseline and proposed values,
signed difference, tolerance, and whether the change falls within that tolerance.
Missing properties are insufficient evidence, not zero. Non-numeric values,
nonfinite values, differences that overflow, and integer conversions that lose
precision are rejected. This is a baseline-retention diagnostic, not a statistical
significance test or a measure of explanatory improvement. Held-out comparison
and application of weight revisions remain experiment-harness work.

`null.contextual-cooccurrence` repeats the fixed-margin endpoint co-presence null
within explicit metadata groups. For example:

```yaml
minimum_observations: 2
strata:
  - {field: source}
  - {field: context, key: genre}
  - {field: context, key: time_bucket}
  - {field: context, key: extractor}
```

All selected categories define each group jointly. Context keys must already be
recorded on observations; time buckets and extraction metadata are never inferred.
String, number, and boolean categories remain distinct. Missing, null, or blank
metadata is reported with excluded observation identities. Each group retains
its observations, counts, and a calculated probability or insufficient-evidence
reading. The result envelope reports assessed group count, including zero when
none qualify. Probabilities are not pooled. This controls only the selected
recorded categories under within-group exchangeability; it neither proves bias
removal nor adjusts for candidate selection or multiple testing.

The first comparator, `compare.scalar-difference`, is selected explicitly in the
profile `comparators` list with empty parameters. The library runs it through
`Engine::compare_measurements` on a tracked before/after measurement pair. Both
measurements must share sensor identity, version, and context. The structured
`Delta` uses the `ScalarDifference` payload: `value` retains before, after, and
`after - before`; `unavailable` retains both original readings; `not_applicable`
rejects structured inputs without scalarizing them. Nonfinite readings and
overflowing differences are errors. Provenance records both measurements and
the exact comparator configuration. Pair selection is explicit; the caller must
ensure comparable coordinate frames. Partition and other comparator families,
profile-wide pairing, and experiment orchestration remain pending.

`compare.kendall` and `compare.rbo` are separate ranking comparators using the
same tracked measurement-pair harness and sensor/version/context checks. Their
structured `RankingComparison` deltas retain both ranking states. Kendall takes
empty parameters and returns normalized discordant-pair distance, discordant
count, and total pair count. It requires complete untied rankings of the same
units and at least two units. This initial comparator is unweighted.

RBO requires `{p: 0.9}` with persistence strictly between zero and one. It compares
equal-depth, nonempty untied prefixes using geometrically weighted overlap plus
the final-depth extrapolated tail. Unit sets may differ; unknown tails remain
explicit and are never filled. RBO reports similarity, whereas Kendall reports
distance. Ties, unresolved identities, unequal RBO depths, and incompatible
Kendall unit sets produce explicit unsupported results. Empty evidence remains
unavailable; malformed repeated or overlapping rank identities are errors.

`compare.jensen-shannon` compares named distributions with explicit
`{normalization: probability}` or `{normalization: mass}` parameters. Probability
inputs must sum to one within 1e-12; mass inputs are normalized by their totals.
Accepted probability totals are also normalized to remove rounding error. Category
names define the shared support; absent categories contribute zero mass, and
explicit zero categories remain visible. The typed `DistributionComparison` delta
retains sorted categories, normalized probabilities, original totals, and symmetric
Jensen–Shannon divergence in bits (0 for identical distributions, 1 for disjoint
support). No transport geometry or smoothing is inferred.

Empty or zero-total distributions are unavailable. Negative or nonfinite weights,
duplicate or empty categories, and overflowing totals are errors. Sparse readings
remain explicit. Sensor, version, and context compatibility checks still apply.

`compare.pairwise-matrix` compares labeled pairwise matrices with
`{minimum_samples: 2}` or a stricter sample floor. It requires identical metrics
and unit axes, plus matching sensor, version, and context. Its `MatrixComparison`
delta retains the full matrix shape, including diagonals, with signed
`after - before` differences only where both cells meet the sample floor. Every
cell retains both original values and sample counts or sparse states. Undefined
and insufficient evidence are never replaced with zero. Empty matrices remain
unavailable; unlabeled matrices are unsupported. No aggregate matrix score is
computed.

`compare.spectrum` derives both spectra from the tracked labeled matrices using
explicit parameters such as `{minimum_samples: 2, tolerance: 1e-12, max_sweeps: 100}`.
It requires matching metrics and axes, with every cell meeting the sample floor.
The typed `SpectralComparison` delta retains both complete eigendecompositions
and signed `after - before` eigenvalue changes in descending eigenvalue order.
Negative eigenvalues remain negative; no explained-variance ratio is inferred.
Missing cells produce an unavailable result, and failure to converge is an error.
This compares spectra, not matched factors or loading distances: different
matrices can have identical spectra, and repeated eigenspaces need not have
unique bases. No aggregate spectral score is computed.

Observation and measurement CLI commands reject comparison and discovery selections; a dedicated
discovery command and experiment execution are still pending.

The storage library also exposes `CandidateRepository`, `ExperimentRepository`,
and `DomainRevisionRepository` through `SeaOrmExperimentRepository`. Candidate
proposals require calculated evidence; completed experiments require experimental
evidence, disjoint observation splits with held-out data, and tracked calculated
deltas. Each write commits its records and provenance atomically. Revision records
reference existing domain versions. These APIs provide storage; candidate
generation and counterfactual execution are not yet exposed in the CLI.

Measurement runs store the exact inferred inputs and domain/frame selectors.
`unclip level verify <run-id>` replays those inputs, recalculates the configured
sensors, and compares the measurements and provenance with the stored results.
Later alternative rankings do not change a recorded selection.

`unclip level profile <profile-id> --table` displays calculated sensor results side
by side, grouped by their full measurement context. Each sensor/version retains
its own values, sample counts, confidence, and sparse states. Multiple readings
are retained; `no result` means that sensor has no entry for the context. Values
from different metrics remain on their own scales. Omit `--table` for YAML, or
use `--format json` for JSON.

`sample`, `compose`, and `export` write `--format yaml`, `json`, or `jsonl`.

Sampling draws candidates by weighted random selection without replacement. Pass
`--seed` to make the selection reproducible, and `--dry-run` to print a packet
without recording usage or saving it. A packet still embeds a wall-clock
`created_at`, so the same seed reproduces the same selection rather than the same
bytes. Seed reproducibility holds within one unclip version. A release that
changes the sampling algorithm may map the same seed to a different selection.

Every packet embeds the query and sampling controls that produced it, so
`unclip replay packet.yaml` re-runs that draw. It uses the recorded seed by
default, or a seed you pass with `--seed`.

`sample` and `compose` differ in how much they hold at once. `sample` streams
candidates page by page into a fixed-size reservoir, so its memory depends on
`--count`, not on how many branches match. `compose` builds one candidate set
per frame slot up front — it has to draw from the same pool for every packet in
a batch — so each slot's scope must match no more than 10,000 branches. Narrow a
slot's `under` scope, or its filters, if `compose` reports that a query matched
too many branches.

`query`, `export --format jsonl`, and `ls` stream too, and read only the columns
they print. `tree` streams as well but renders a row per path in the subtree, so
it holds them all and stops at 100,000 paths — pick a deeper root if it reports
a scope that matched too many.

## Quality and release checks
- `cargo fmt --all --check`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps`
- `cargo test --locked --workspace --all-targets`
- `cargo build --release --locked --workspace`
- `cargo package --locked --workspace`

CI:
- `cargo deny check advisories bans sources licenses`
- `cargo llvm-cov --locked --workspace --all-targets --summary-only --fail-under-lines 85`

## License

MIT. See `LICENSE`.
