# Development Phase Policies

## Branching strategy

- `main` — stable, releasable code only. Merge from `develop` when stable.
- `develop` — active development branch. All day-to-day work goes here.

Push regularly. Never commit red code. Run `just gate` before every commit.

---

## UI sync invariant

Both the terminal UI and the web UI must render every `OmegaEvent` variant —
enforced at compile time by exhaustive switch statements in
`src/terminal/app.ts` and `src/web/client/App.tsx`.
