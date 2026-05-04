/**
 * Phase 3.1 — Leptos smoke test.
 *
 * Visits `/leptos/`, waits for the wasm bundle to load, the WebSocket
 * to open, and the first frame the server emits (`ready`) to be
 * deserialised through `WsMessage` and applied to the reactive
 * `SessionStore`. The pretty-printed JSON snapshot under
 * `[data-testid="leptos-debug-store"]` then contains
 * `"connected": true`.
 *
 * This locks in the wiring across the full stack:
 *   - omega-server's `/leptos/` `ServeDir` mount
 *   - trunk's `public_url = "/leptos/"` asset paths
 *   - the wasm bundle's `WebSocket::new("ws://…/ws")` connect
 *   - typed `WsMessage` deserialisation in `protocol.rs`
 *   - `SessionStore::apply` reducing each frame in `store.rs`
 *
 * The fixture binary used here is `mock-omega-server` (port 3003),
 * built by `just rust-build-mock-server`. It serves the Leptos bundle
 * from `frontends/leptos/dist` (built by `just web-leptos-build`).
 *
 * Lifespan: this spec is deleted in Phase 3.7 alongside the rest of
 * Playwright when chromiumoxide takes over.
 */

import { test, expect } from "@playwright/test";

test("leptos: /leptos/ loads, WS connects, ready frame updates the store", async ({ page }) => {
  await page.goto("/leptos/");

  // The store starts disconnected; once the WS opens and the server
  // emits `ready`, `SessionStore::apply(WsMessage::Ready)` flips
  // `connected` to true and the JSON dump reflects it.
  const storeDump = page.getByTestId("leptos-debug-store");
  await expect(storeDump).toContainText('"connected": true', { timeout: 5000 });
  // `transportErrors` must remain empty — there's no malformed-frame path.
  await expect(storeDump).toContainText('"transportErrors": []');
});

test("leptos: bare /leptos redirects to /leptos/", async ({ request }) => {
  // request.get follows redirects by default; check the final URL.
  const resp = await request.get("/leptos", { maxRedirects: 0, failOnStatusCode: false });
  expect(resp.status()).toBe(308);
  expect(resp.headers()["location"]).toBe("/leptos/");
});
