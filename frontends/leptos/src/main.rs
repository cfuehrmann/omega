//! Phase 3.6 \u2014 thin binary entrypoint.
//!
//! All real code lives in `omega_web::*`. The lib/bin split lets
//! host-target snapshot tests (`tests/snapshots.rs`) pull in
//! components without building the bin path.

fn main() {
    omega_web::run();
}
