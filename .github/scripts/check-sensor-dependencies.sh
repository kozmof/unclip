#!/usr/bin/env bash
# Hold `unclip-sensors` to a deterministic dependency set.
#
# A sensor must produce the same output for the same evidence, byte for byte.
# `conformance::assert_sensor` runs one sensor twice and compares, which catches
# a sensor that is nondeterministic on the inputs a test happens to supply; this
# catches the whole class, by denying the crate any means of observing a clock,
# an RNG, the network, or the host at all.
#
# This is an ALLOWLIST, and that is the point. It used to be a denylist of
# effectful crate names, which fails open: it named `rand`, `chrono` and
# `tokio`, and would have admitted `uuid` (v4 draws from the OS), `ahash` and
# `indexmap` (randomly seeded hashers whose iteration order varies per process),
# `instant`, `web-time`, `fastrand`'s next rename, or any future crate nobody
# thought to list. An allowlist fails closed: a new dependency is refused until
# someone states that it cannot make a sensor nondeterministic, which is the
# decision worth forcing.
#
# Adding an entry means answering one question in the PR that adds it: can this
# crate observe anything that differs between two runs on the same inputs? If it
# can, it does not belong here whatever else it offers.
set -euo pipefail

# Permitted normal (non-dev) dependencies of unclip-sensors, transitively.
# Grouped by why each is deterministic.
ALLOWED=(
    # The crate under test.
    unclip-sensors
    # First-party layers below it: data, calculation, and plugin contracts.
    # Each is itself covered by this check through its own place in the tree.
    unclip-domain
    unclip-epistemic
    unclip-measure
    unclip-observe
    unclip-plugin
    # Serialization and numeric formatting: pure functions of their input.
    serde
    serde_core
    serde_derive
    serde_json
    itoa
    memchr
    zmij
    # Version parsing and comparison: pure.
    semver
    # Error derive: compile-time only.
    thiserror
    thiserror-impl
    # async fn in traits: a desugaring, no runtime and no executor.
    async-trait
    # Proc-macro machinery, used only at build time.
    proc-macro2
    quote
    syn
    unicode-ident
)

dependency_tree=$(
    cargo tree --locked --color never -p unclip-sensors \
        --edges normal --prefix none --format '{p}' --target all
)

unexpected=$(
    printf '%s\n' "$dependency_tree" |
        awk '{print $1}' |
        grep -v '^$' |
        sort -u |
        grep -vxF "$(printf '%s\n' "${ALLOWED[@]}")" || true
)

if [ -n "$unexpected" ]; then
    {
        echo "unclip-sensors gained dependencies that are not on the deterministic allowlist:"
        printf '  %s\n' $unexpected
        echo
        echo "A sensor must be reproducible byte for byte. If this crate cannot observe"
        echo "a clock, an RNG, the network, or anything else that differs between two runs"
        echo "on the same inputs, add it to ALLOWED in $0 and say why in the PR."
    } >&2
    exit 1
fi
