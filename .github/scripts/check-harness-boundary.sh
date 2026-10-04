#!/usr/bin/env bash
# Keep plugin implementations from minting their own epistemic labels.
#
# A plugin receives the token for the one operation it performs — a sensor a
# `CalculationToken`, an inferrer an `InferenceToken` — and the harness that
# hands it over stamps provenance. Two escape hatches exist for that harness:
# `EmitToken::from_harness`, which mints a token of any operation, and the
# `Tracked` constructors that attach an operation label to an arbitrary value.
# The type system cannot keep them from other crates: `unclip-plugin` itself
# mints tokens for its contexts, and every plugin crate depends on it, so a
# Cargo feature would be unified on for them too. This check holds the line
# instead.
#
# It scans the plugin-implementation crates' library code, stopping at the
# first `#[cfg(test)]` in each file, since tests legitimately build fixtures
# through the harness constructors. A plugin that needs one in production code
# is asking for an operation it was not handed, which is the decision worth
# forcing into review.
set -euo pipefail

PLUGIN_CRATES=(
    crates/unclip-sensors
    crates/unclip-infer
    crates/unclip-interpret
)

PATTERN='from_harness|Tracked::from_(calculated|inferred|stored|shared|derived|derived_shared)\b'

violations=$(
    find "${PLUGIN_CRATES[@]/%//src}" -name '*.rs' -print0 |
        xargs -0 awk -v pattern="$PATTERN" '
            FNR == 1 { in_tests = 0 }
            /^[[:space:]]*#\[cfg\(test\)\]/ { in_tests = 1 }
            !in_tests && $0 ~ pattern { print FILENAME ":" FNR ": " $0 }
        '
)

if [ -n "$violations" ]; then
    {
        echo "plugin crates must emit through the token they are handed, not mint labels:"
        printf '%s\n' "$violations"
        echo
        echo "Take the operation's token from the plugin trait method instead. If a new"
        echo "harness genuinely needs this, it belongs in unclip-plugin or unclip-engine."
    } >&2
    exit 1
fi
