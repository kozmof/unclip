#[test]
fn epistemic_boundaries_reject_invalid_programs() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
