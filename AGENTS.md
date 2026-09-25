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

<!-- BEGIN DEVRITES CODEX -->
## DevRites For Codex

This project has DevRites installed for both Claude Code and Codex.

## Codex usage

- **Inspect before trust.** Codex skips project-scoped `.codex/` configuration and agents in an untrusted project. Before enabling them, inspect `.codex/config.toml`, `.codex/agents/`, and the project guidance. The human operator decides whether to trust the folder.
- DevRites workflow skills are available to Codex from `.agents/skills`.
- Use `$rite` or `$rite-<verb>` through Codex skills, or open `/skills` and select the matching DevRites skill.
- If the user mentions a DevRites slash command such as `/rite spec`, `/rite-build`, or `/rite-seal`, treat that as an explicit request to use the corresponding DevRites skill.
- DevRites runtime helpers run through the installed `devrites-engine` binary.
- Before using any DevRites workflow skill, read `.agents/skills/devrites-lib/reference/standards/core.md`. Load other `.agents/skills/devrites-lib/reference/standards/*.md` files when the skill or rule index asks for them. These are DevRites engineering standards, not Codex exec-policy `.rules` files.
- Installed `.agents/` mirrors may be Git-ignored. If a repository-aware file tool refuses an ignored path, read it with a native filesystem command instead; a tool refusal is not a completed task.
- For automatic Engram calls, omit optional `project` and `session_id` unless an exact value came from Engram or repository configuration. Never derive either from `task_name`, a run ID, directory name, or normalized slug. Call `mem_session_summary` without them by default; on `unknown_session` or `unknown_project`, retry once with both optional fields omitted. If auto-detection is ambiguous, ask the user instead of guessing.
- Custom Codex subagents generated from the DevRites review agents live in `.codex/agents`.
- In DevRites guidance, **invoke** means run a skill inline in the current context; **dispatch** means start a fresh agent with `spawn_agent`, wait for it, and reconcile its result.
- Dispatch every exact named `devrites-<role>` required by the active workflow in a fresh subagent thread, wait for it, and reconcile its result. If that role is unavailable, stop for HITL; never skip it, never substitute a generic/default child, and never execute the specialist role in the root context.
- The root uses the workspace-capable `devrites-orchestrator` profile because Codex children cannot elevate above the parent permission ceiling. This permission is for native writer dispatch and exact path-bounded executable workflow artifacts under the active `.devrites/work/<slug>/`; the root must never edit product source or tests itself.
- Every generated specialist is hook-free. `devrites-slice-wright` alone uses `default_permissions = ":workspace"`; every other specialist uses `default_permissions = ":read-only"`. Exact paths are instruction-enforced: put the project-relative paths in the task, wait for the wright, compare its file list and `git diff --name-only` with that contract, and reject any extra path. Never bypass the wright, widen its contract, or edit source in the root; the root must never recreate an engine dispatch bridge.
- A seal GO, AFK mode, or autocomplete flag never authorizes an irreversible action. Disclose the exact commit/push/tag/PR plan and obtain fresh explicit user approval for that attempt; any changed or retried plan needs fresh approval. Native host permission and sandbox prompts remain authoritative and cannot be inferred or bypassed.

## Workflow contract

- Keep all feature state in `.devrites/work/<slug>/` and preserve `.devrites/ACTIVE`.
- Follow the DevRites lifecycle: frame -> spec -> clarify -> temper -> define -> plan -> vet -> build -> converge -> prove -> polish -> review -> seal -> ship -> done.
- Claims of completion need recorded evidence in the feature workspace, not confidence alone.
<!-- END DEVRITES CODEX -->

<!-- BEGIN DEVRITES PI -->
## DevRites For pi

This project has DevRites installed for pi.

## pi usage

- DevRites workflow skills live in `.pi/skills`. Public commands run as `/rite`, `/rite-spec`, ... (prompt templates under `.pi/prompts`) or `/skill:rite`, `/skill:rite-spec`, ...; the forms are equivalent.
- Before using any DevRites workflow skill, read `.pi/skills/devrites-lib/reference/standards/core.md`. Load other `.pi/skills/devrites-lib/reference/standards/*.md` files when the skill or rule index asks for them.
- DevRites specialist agents live in `.pi/agents` and are provided by the `pi-subagents` extension. Before dispatch, run `subagent({ action: "list" })`; if a required `devrites-<role>` is absent or not executable, stop for HITL — never skip it, substitute a generic child, or run the specialist role in the root context.
- In DevRites guidance, **invoke** means run a skill inline in the current context; **dispatch** means start a fresh agent with `subagent({ agent, task })` (or `runs.run`/`runs.all` in a `workflowScript`), wait for it, and reconcile its result.
- Only `devrites-slice-wright` may edit source or tests; every other specialist is read-only by tool allowlist. Exact paths are instruction-enforced: put the project-relative paths in the task, wait for the wright, compare its file list and `git diff --name-only` with that contract, and reject any extra path.
- DevRites runtime helpers run through the installed `devrites-engine` binary.
- Installed `.pi/` content loads only after the project is trusted. If pi has not trusted this project, the skills, agents, and prompts above are not active.
- A seal GO, AFK mode, or autocomplete flag never authorizes an irreversible action. Disclose the exact commit/push/tag/PR plan and obtain fresh explicit user approval for that attempt; any changed or retried plan needs fresh approval.

## Workflow contract

- Keep all feature state in `.devrites/work/<slug>/` and preserve `.devrites/ACTIVE`.
- Follow the DevRites lifecycle: frame -> spec -> clarify -> temper -> define -> plan -> vet -> build -> converge -> prove -> polish -> review -> seal -> ship -> done.
- Claims of completion need recorded evidence in the feature workspace, not confidence alone.
<!-- END DEVRITES PI -->

<!-- BEGIN DEVRITES DEVIN -->
## DevRites For Devin

This project has DevRites installed for Devin CLI.

## Devin usage

- DevRites workflow skills live in `.devin/skills`. Run them as `/rite`, `/rite-spec`, and the other `/rite-*` slash commands.
- Before using any DevRites workflow skill, read `.devin/skills/devrites-lib/reference/standards/core.md`. Load other `.devin/skills/devrites-lib/reference/standards/*.md` files when the skill or rule index asks for them.
- DevRites specialist agents live in `.devin/agents` as custom subagent profiles offered to `run_subagent`. If a required `devrites-<role>` profile is not among the offered profiles, stop for HITL - never skip the role, never substitute `subagent_general`, `subagent_explore`, or another generic profile, never inline the specialist prompt, and never execute the specialist role in the root context.
- In DevRites guidance, **invoke** means run a skill inline in the current context; **dispatch** means start a fresh agent with `run_subagent` using the exact `profile` name, wait for it with `read_subagent`, and reconcile its result.
- Only `devrites-slice-wright` may edit source or tests; every other specialist is read-only by `allowed-tools`. Exact paths are instruction-enforced: put the project-relative paths in the task, wait for the wright, compare its file list and `git diff --name-only` with that contract, and reject any extra path.
- Custom profiles do not inherit interactive tool grants, and background subagents auto-deny unapproved tools. Dispatch the write-capable wright in the foreground, or make sure its `edit`, `write`, and `exec` tools are approved first. Specialists cannot use `ask_user_question`; the root relays user questions and answers through the task text and returned result.
- DevRites runtime helpers run through the installed `devrites-engine` binary.
- Skills and agent profiles are loaded when a session starts. If they were installed while this session was open, restart the session or reopen the project before relying on them.
- A seal GO, AFK mode, or autocomplete flag never authorizes an irreversible action. Disclose the exact commit/push/tag/PR plan and obtain fresh explicit user approval for that attempt; any changed or retried plan needs fresh approval.

## Workflow contract

- Keep all feature state in `.devrites/work/<slug>/` and preserve `.devrites/ACTIVE`.
- Follow the DevRites lifecycle: frame -> spec -> clarify -> temper -> define -> plan -> vet -> build -> converge -> prove -> polish -> review -> seal -> ship -> done.
- Claims of completion need recorded evidence in the feature workspace, not confidence alone.
<!-- END DEVRITES DEVIN -->
