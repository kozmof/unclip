# unclip

unclip is a command-line tool for getting varied output from LLMs by building
the possibility space outside the model. Store ideas as addressable branches,
index and constrain them, then sample structured selections to feed the model.

## Why

Ask a small or local LLM to invent a whole search space and it tends to return
the same safe answers.

```txt
Tokyo story        → cafe, rain, lonely person
debug advice       → add logs, check config
research critique  → baseline, limitation, future work
```

The problem is usually a narrow search space, not model size. unclip moves that
search space out of the model. Build a structured archive of possibilities,
sample from it under constraints, and hand the model concrete material to work
with instead of asking it to imagine everything.

## Core concepts

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

## Quality and release checks

The workspace is set up for release-minded source distribution. CI runs these
checks.

- `cargo fmt --all --check`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps`
- `cargo test --locked --workspace --all-targets`
- `cargo build --release --locked --workspace`
- `cargo package --locked --workspace`
- `cargo deny check advisories bans sources licenses`
- `cargo llvm-cov --locked --workspace --all-targets --summary-only --fail-under-lines 85`

These checks cover formatting, linting, docs, tests, release builds, package
verification, dependency policy, and line coverage before changes land on main.

## Workspace layout

```txt
crates/
  unclip-core/      Domain model and validation, no persistence deps.
  unclip-entity/    SeaORM entities.
  unclip-migration/ Database migrations.
  unclip-store/     Repository traits, SeaORM implementations, and mappers.
  unclip-sample/    Seeded weighted sampling.
  unclip-io/        YAML, JSON, and JSONL parsing and rendering.
  unclip-match/     daachorse-based pattern matcher.
  unclip-cli/       The `unclip` command-line interface.
```

## License

MIT. See `LICENSE`.
