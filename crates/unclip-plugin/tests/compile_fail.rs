//! Exact-diagnostic assertions for the epistemic type boundary.
//!
//! Each case pins the full rendered compiler error, which is what makes these
//! stronger than a bare `compile_fail` doctest: the program must fail for the
//! stated reason, not merely fail.
//!
//! That strength is also the constraint on what belongs here. A diagnostic
//! that quotes the standard library — for example the list of `From` impls
//! rustc prints for `f64` — changes between toolchains and would fail this
//! suite for reasons unrelated to the invariant. Those invariants live in
//! `unclip-measure` as `compile_fail` doctests pinned to an error code
//! instead, which asserts the specific bound without the rendered text.
//!
//! Regenerate expectations after an intentional change with
//! `TRYBUILD=overwrite cargo test -p unclip-plugin --test compile_fail`.

#[test]
fn epistemic_boundaries_reject_invalid_programs() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
