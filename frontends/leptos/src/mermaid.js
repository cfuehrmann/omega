// Phase 3.6 — mermaid lazy-load shim for the Leptos UI.
//
// Mirrors `src/web/client/App.tsx::renderMermaidBlocks` exactly so
// the Playwright selectors that lock down the SolidJS UI's mermaid
// surface (mermaid-wrapper, mermaid-diagram, mermaid-error-notice,
// mermaid-source, code-copy-btn) work verbatim against the Leptos
// UI too.
//
// Loaded by trunk via `<link data-trunk rel="copy-file">` in
// `index.html` and pulled in from Rust via `wasm_bindgen(module = ...)`.
// Mermaid itself is loaded **only** when a `pre.mermaid-pending`
// element actually appears — keeps the wasm bundle delta at zero
// and adds the ~600 KB mermaid bundle as page weight only when it
// is really needed.

let _mermaid = null;
let _mermaidInitialised = false;
let _mermaidCounter = 0;

async function getMermaid() {
  if (!_mermaid) {
    // Resolved by the browser at runtime; we expect mermaid to be
    // available either as an ESM module on the page (preferred) or
    // via a CDN URL the page hosts. If neither exists, the import
    // throws and renderMermaid falls through to the error path.
    const mod = await import("https://cdn.jsdelivr.net/npm/mermaid@11/+esm");
    _mermaid = mod.default;
  }
  if (!_mermaidInitialised) {
    _mermaid.initialize({ startOnLoad: false, theme: "dark" });
    _mermaidInitialised = true;
  }
  return _mermaid;
}

/**
 * Inject a copy button identical to the SolidJS UI's `addCopyButton`.
 * Same `code-copy-btn` class + data-testid for Playwright parity.
 */
function addCopyButton(parent, textToCopy) {
  const btn = document.createElement("button");
  btn.className = "code-copy-btn";
  btn.setAttribute("data-testid", "code-copy-btn");
  btn.setAttribute("aria-label", "copy code");
  btn.textContent = "copy";
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    navigator.clipboard.writeText(textToCopy).then(() => {
      btn.textContent = "copy ✓";
      setTimeout(() => { btn.textContent = "copy"; }, 1500);
    }).catch(() => {
      btn.textContent = "err";
      setTimeout(() => { btn.textContent = "copy"; }, 1500);
    });
  });
  parent.appendChild(btn);
}

/**
 * Find every `pre.mermaid-pending` inside `container`, render each
 * through mermaid, and replace the `<pre>` with a wrapper carrying
 * the SVG (or an error notice + raw source on failure).
 *
 * Mirrors `App.tsx::renderMermaidBlocks` byte-for-byte on the
 * `data-testid` surface so the existing Playwright spec
 * `e2e/web-ui-mermaid.spec.ts` ports to a Leptos counterpart with
 * zero selector edits.
 */
export async function renderMermaid(container) {
  if (!container) return;
  const blocks = Array.from(
    container.querySelectorAll("pre.mermaid-pending"),
  );
  if (blocks.length === 0) return;

  // Remove the marker class synchronously before any await so a
  // second call from a concurrent re-render cannot pick up the
  // same elements.
  blocks.forEach((pre) => pre.classList.remove("mermaid-pending"));

  let mermaid;
  try {
    mermaid = await getMermaid();
  } catch (err) {
    for (const pre of blocks) {
      const wrapper = document.createElement("div");
      wrapper.className = "mermaid-wrapper mermaid-error";
      wrapper.setAttribute("data-testid", "mermaid-wrapper");
      const notice = document.createElement("div");
      notice.className = "mermaid-error-notice";
      notice.setAttribute("data-testid", "mermaid-error-notice");
      notice.textContent = `⚠ Mermaid error: ${
        err instanceof Error ? err.message : String(err)
      }`;
      wrapper.appendChild(notice);
      const sourcePre = document.createElement("pre");
      sourcePre.className = "mermaid-source";
      sourcePre.setAttribute("data-testid", "mermaid-source");
      sourcePre.textContent = pre.dataset.mermaidSource ?? pre.textContent ?? "";
      wrapper.appendChild(sourcePre);
      pre.replaceWith(wrapper);
    }
    return;
  }

  for (const pre of blocks) {
    const source = pre.dataset.mermaidSource ?? pre.textContent ?? "";
    const id = `mermaid-svg-${++_mermaidCounter}`;

    const wrapper = document.createElement("div");
    wrapper.className = "mermaid-wrapper";
    wrapper.setAttribute("data-testid", "mermaid-wrapper");

    addCopyButton(wrapper, source);

    try {
      const { svg } = await mermaid.render(id, source);
      const diagram = document.createElement("div");
      diagram.className = "mermaid-diagram";
      diagram.setAttribute("data-testid", "mermaid-diagram");
      diagram.innerHTML = svg;
      wrapper.appendChild(diagram);
    } catch (err) {
      wrapper.classList.add("mermaid-error");
      const notice = document.createElement("div");
      notice.className = "mermaid-error-notice";
      notice.setAttribute("data-testid", "mermaid-error-notice");
      notice.textContent = `⚠ Mermaid error: ${
        err instanceof Error ? err.message : String(err)
      }`;
      wrapper.appendChild(notice);
      const sourcePre = document.createElement("pre");
      sourcePre.className = "mermaid-source";
      sourcePre.setAttribute("data-testid", "mermaid-source");
      sourcePre.textContent = source;
      wrapper.appendChild(sourcePre);
    }

    pre.replaceWith(wrapper);
  }
}

/**
 * Inject a copy button on each `<pre>` in `container` that does not
 * already carry one (data-enhanced="1"). Skips mermaid-pending
 * blocks because their button is added by the wrapper later.
 *
 * Mirrors `App.tsx::enhanceCodeBlocks` minus the diff-rendering
 * branch (which the Rust side handles by setting innerHTML to the
 * pre-rendered diff spans before this function is called).
 */
export function addCopyButtons(container) {
  if (!container) return;
  const pres = Array.from(container.querySelectorAll("pre"));
  for (const pre of pres) {
    if (pre.dataset.enhanced) continue;
    pre.dataset.enhanced = "1";
    if (pre.classList.contains("mermaid-pending")) continue;
    const code = pre.querySelector("code");
    const text = code?.textContent ?? pre.textContent ?? "";
    addCopyButton(pre, text);
  }
}
