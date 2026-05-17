//! Port of `e2e/leptos-smoke.spec.ts`.
//!
//! Phase 4 Q7 reduced this file to one case: the `/leptos` → `/leptos/`
//! redirect alias was removed alongside the Trunk `public_url` flip to
//! `/`, so the second Playwright case has no Rust counterpart.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::time::Duration;

use omega_e2e::TestHarness;

/// `loads at /, marks the page connected when WS attaches`.
///
/// Mirrors the (pre-Q7) Playwright case: navigate to the site root,
/// wait for the `ready` frame to flip `<main data-connected="true">`.
/// The harness constructor already does this, but we re-assert here
/// so the invariant is owned by the test and shows up in the trace if
/// it regresses.
#[tokio::test]
#[ignore = "browser"]
async fn leptos_smoke_loads_connects() {
    let h = TestHarness::launch().await.expect("launch harness");
    h.wait_for_attr("main", "data-connected", "true", Duration::from_secs(5))
        .await
        .expect("data-connected=true");
}
