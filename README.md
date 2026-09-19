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
lists to empty. No built-in generator or null-model algorithm is registered yet;
this configuration support does not run discovery or experiments.

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
