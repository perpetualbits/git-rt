//! Run the seeded xterm/ECMA-48 spec-case table against the vendored oracle. When
//! `vt-term` lands, the identical table runs against it (`run_spec_cases::<VtTerm>`),
//! turning this into the conformance gate for the in-house engine.

use vt_conformance::spec::{cases, run_spec_cases};
use vt_conformance::vendored::Vendored;

#[test]
fn vendored_passes_the_spec_cases() {
    let fails = run_spec_cases::<Vendored>(&cases());
    assert!(fails.is_empty(), "spec-case failures ({}):\n{}", fails.len(), fails.join("\n"));
}
