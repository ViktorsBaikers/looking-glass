# AGENTS.md

Looking Glass: self-hosted network diagnostics console. Rust workspace (`crates/central`, `crates/agent`, `crates/shared`; axum + tokio) serving a SvelteKit SPA (`frontend/`, Svelte 5, Tailwind v4 + `tailwind-variants`).

**Tradeoff:** these rules bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think before coding

**State assumptions. Surface confusion. Name tradeoffs.**

- State your assumptions explicitly. If uncertain, ask.
- Multiple interpretations → present them; let the user pick.
- A simpler approach exists → say so. Push back when warranted.
- Something unclear → stop, name what's confusing, ask.

## 2. Simplicity first

**Minimum code that solves the problem. Nothing speculative.**

- Build only what was asked.
- Inline single-use code; add an abstraction when a second caller exists.
- Handle errors that can actually occur.
- 200 lines that could be 50 → rewrite it.

Test: would a senior engineer call this overcomplicated? If yes, simplify.

## 3. Surgical changes

**Touch only what you must. Clean up only your own mess.**

- Match existing style, even if you'd do it differently.
- Leave adjacent code, comments, and formatting as they are; mention unrelated dead code instead of deleting it.
- Remove imports, variables, and functions that *your* change made unused.

Test: every changed line traces directly to the user's request.

## 4. Goal-driven execution

**Define success criteria. Loop until verified.**

Turn tasks into verifiable goals:

- "Add validation" → write tests for invalid inputs, then make them pass.
- "Fix the bug" → write a test that reproduces it, then make it pass.
- "Refactor X" → tests pass before and after.

For multi-step work, state a brief plan:

```text
1. [Step] → verify: [check]
2. [Step] → verify: [check]
```

## Project gotchas

- `central` embeds `frontend/build` via rust-embed: build the SPA before any cargo command. The `Makefile` targets enforce this; prefer `make test`, `make clippy`, `make verify` over raw cargo.
- Frontend: `npm test` (vitest), `npm run test:e2e` (playwright), `npm run check` (svelte-check), all from `frontend/`.

## Code intelligence

- **Codebase Memory** for discovery: architecture, call paths, blast radius, finding symbols whose names you don't know.
- **Serena / LSP** for exact symbol work: definitions, references, implementations, diagnostics, renames, safe deletion. Activate the repo as the Serena project first.
- Plain read/edit for known files, config, docs, and small line edits.

## Agent skills

### Issue tracker

Issues live in GitHub Issues on `ViktorsBaikers/looking-glass` (via `gh` CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `CONTEXT.md` + `docs/adr/`. See `docs/agents/domain.md`.
