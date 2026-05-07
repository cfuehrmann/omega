//! Port of `e2e/leptos-smoke.spec.ts` (2 cases).

use std::time::Duration;

use omega_e2e::TestHarness;

/// `loads at /leptos/, marks the page connected when WS attaches`.
///
/// Mirrors the Playwright case: navigate to `/leptos/`, wait for the
/// `ready` frame to flip `<main data-connected="true">`. The harness
/// constructor already does this, but we re-assert here so the
/// invariant is owned by the test and shows up in the trace if it
/// regresses.
#[tokio::test]
#[ignore = "browser"]
async fn leptos_smoke_loads_connects() {
    let h = TestHarness::launch().await.expect("launch harness");
    h.wait_for_attr("main", "data-connected", "true", Duration::from_secs(5))
        .await
        .expect("data-connected=true");
}

/// `bare /leptos redirects to /leptos/ (308)` — mirrors the
/// Playwright HTTP-level check (no page navigation needed).
#[tokio::test]
#[ignore = "browser"]
async fn leptos_smoke_bare_redirects_to_trailing_slash() {
    let h = TestHarness::launch().await.expect("launch harness");
    let raw_url = format!("{}/leptos", h.base_url);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build no-redirect client");
    let resp = client.get(&raw_url).send().await.expect("GET /leptos");
    assert_eq!(resp.status().as_u16(), 308, "/leptos must 308-redirect");
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(
        location, "/leptos/",
        "Location header must be exactly /leptos/"
    );
}
