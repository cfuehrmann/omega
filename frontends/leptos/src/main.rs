//! Phase 3.6 \u2014 thin binary entrypoint.
//!
//! All real code lives in `omega_web::*`. The lib/bin split lets
//! host-target snapshot tests (`tests/snapshots.rs`) pull in
//! components without building the bin path.

#[mutants::skip] // WASM binary entry point; wasm_bindgen_test runner never calls main().
fn main() {
    omega_web::run();
}
