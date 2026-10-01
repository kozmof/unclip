#!/usr/bin/env bash
# Verify that every crate in the workspace packages and builds from its tarball.
#
# This wraps `cargo package --locked --workspace` because the bare command is
# not reproducible here, and fails in the direction that hides bugs.
#
# Workspace packaging rewrites each internal `path` dependency into a registry
# dependency and republishes all eighteen crates into a throwaway registry under
# `$CARGO_TARGET_DIR/package/tmp-registry`, so verifying `unclip-measure` means
# building it against a registry copy of `unclip-domain 0.1.0` rather than
# against the sources next door. Cargo treats a registry source as immutable: an
# extracted copy under `$CARGO_HOME/registry/src` is keyed by name and version
# alone and is never re-extracted, and the rlib built from it is reused on the
# same grounds. That assumption holds for crates.io, where 0.1.0 is one
# immutable tarball forever. It is false for this registry, where every run
# republishes different code under the same unchanging version.
#
# The result is that verification silently compiles this commit's crates against
# whatever the internal API looked like the last time the step ran. It fails on
# any commit that changes an internal API — the symptom is a crate failing to
# compile against a sibling it compiles against perfectly well in `cargo build`
# — and, worse, it can pass on a stale cache for a commit that would not build
# on a clean machine. Either way the check is not testing this commit.
#
# So drop both caches first: the extracted sources and cached tarballs for our
# own crates, and the whole verification target directory. Only `unclip-*`
# entries are removed, so no third-party crate is re-downloaded. Packaging
# builds every crate from scratch regardless, so the separate target directory
# costs nothing beyond disk and keeps the shared `target/` out of it.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

cargo_home=${CARGO_HOME:-$HOME/.cargo}
target_dir=target/package-verify

rm -rf "$cargo_home"/registry/src/*/unclip-*
rm -f "$cargo_home"/registry/cache/*/unclip-*.crate
rm -rf "$target_dir"

CARGO_TARGET_DIR=$target_dir exec cargo package --locked --workspace "$@"
