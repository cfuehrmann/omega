/**
 * Phase 3.0 — Leptos smoke test.
 *
 * Visits `/leptos/`, waits for the wasm bundle to load, the WebSocket
 * to open, and the first frame the server emits (`ready`) to be
 * rendered as an `<li>` inside `[data-testid="leptos-frames"]`.
 *
 * This locks in the wiring across the full stack:
 *   - omega-server's second `ServeDir` mount (`/leptos/`)
 *   - trunk's `public_url = "/leptos/"` asset paths
 *   - the wasm bundle's `WebSocket::new("ws://…/ws")` connect
 *   - the frame-type dispatch path inside `App`
 *
 * The fixture binary used here is `mock-omega-server` (port 3003),
 * built by `just rust-build-mock-server`. It serves the Leptos bundle
 * from `frontends/leptos/dist` (built by `just web-leptos-build`).
 *
 * Lifespan: this spec is deleted in Phase 3.7 alongside the rest of
 * Playwright when chromiumoxide takes over.
 */

import { test, expect } from "@playwright/test";

test("leptos: /leptos/ loads, WS connects, ready frame renders", async ({ page }) => {
  await page.goto("/leptos/");

  // The list starts empty; once the WS opens and the server emits its
  // initial `ready` frame, an <li>ready</li> appears.
  const frames = page.getByTestId("leptos-frames");
  await expect(frames.locator("li", { hasText: /^ready$/ })).toBeVisible({ timeout: 5000 });
  // Sanity: the running counter increments in step with the list.
  await expect(page.getByTestId("leptos-status")).toHaveText(/frames seen: [1-9]/);
});

test("leptos: bare /leptos redirects to /leptos/", async ({ request }) => {
  // request.get follows redirects by default; check the final URL.
  const resp = await request.get("/leptos", { maxRedirects: 0, failOnStatusCode: false });
  expect(resp.status()).toBe(308);
  expect(resp.headers()["location"]).toBe("/leptos/");
});
