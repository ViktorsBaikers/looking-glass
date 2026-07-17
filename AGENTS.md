<!-- BEGIN DEVRITES CODEX -->
## DevRites For Codex

This project has DevRites installed for both Claude Code and Codex.

## Codex usage

- DevRites workflow skills are available to Codex from `.agents/skills`.
- Use `$rite` or `$rite-<verb>` through Codex skills, or open `/skills` and select the matching DevRites skill.
- If the user mentions a DevRites slash command such as `/rite spec`, `/rite-build`, or `/rite-seal`, treat that as an explicit request to use the corresponding DevRites skill.
- DevRites runtime helpers are mirrored for Codex under `.agents/skills/devrites-lib/scripts/`.
- Before using any DevRites workflow skill, read `.agents/devrites/rules/core.md`. Load other `.agents/devrites/rules/*.md` files when the skill or rule index asks for them. These are DevRites engineering rules, not Codex exec-policy `.rules` files.
- Custom Codex subagents generated from the DevRites review agents live in `.codex/agents`.
- When DevRites skill prose asks for a DevRites specialist or writer agent, use the matching Codex custom agent from `.codex/agents/devrites-*.toml` through Codex subagents. If Codex subagents are unavailable in the current surface, run the skill's documented inline fallback and say that the result was not an independent subagent review.
- Claude Code agent hook metadata is not active in Codex. The generated Codex agents preserve read-only intent with Codex sandbox settings where possible; still follow DevRites' scope and no-mutation rules explicitly.

## Workflow contract

- Keep all feature state in `.devrites/work/<slug>/` and preserve `.devrites/ACTIVE`.
- Follow the DevRites lifecycle: spec -> define -> build -> prove -> polish -> review -> seal -> ship.
- Claims of completion need recorded evidence in the feature workspace, not confidence alone.
<!-- END DEVRITES CODEX -->

## Code intelligence and tool routing

This repository provides both Codebase Memory MCP and Serena. Use them for
different purposes rather than issuing duplicate queries to both.

### Use Codebase Memory first for

- repository architecture and subsystem discovery;
- package, module and service relationships;
- broad semantic search when the symbol name is unknown;
- call-path and dependency tracing;
- cross-service and infrastructure relationships;
- change-impact and blast-radius analysis;
- identifying likely files and symbols involved in a feature.

### Use Serena when language-server or symbol-level accuracy is needed

- locating an exact symbol declaration or definition;
- finding exact references to a symbol;
- finding implementations of an interface, trait or abstract type;
- inspecting symbol or file diagnostics;
- understanding symbol relationships that text search cannot establish reliably;
- performing a project-wide symbol rename;
- validating whether deleting or changing a symbol is safe;
- symbol-level edits and refactors.

### Required Serena behavior

- At the beginning of a coding task, ensure the current repository is activated
  as the Serena project before using Serena tools.
- Prefer Serena over grep or repository-wide text search when the request
  concerns exact declarations, references, implementations, diagnostics,
  renaming or safe symbol removal.
- Do not use Serena merely to read a small known file or make a trivial
  line-level edit.
- Do not duplicate a successful Codebase Memory query with Serena unless exact
  language-server confirmation is needed.
- Before a non-trivial cross-file refactor, use Serena to inspect references and
  implementations.
- After a symbol rename or structural refactor, use Serena diagnostics and then
  run the repository's normal tests and type checks.

### Use native Codex tools for

- reading known files;
- small localized edits;
- applying patches;
- ordinary directory and filename inspection;
- configuration, documentation and non-code files.

Use the raw underlying command when exact output, a complete stack trace or
untruncated diagnostic information is required.
