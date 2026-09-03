# Agent tooling

How this repo instructs and gates coding agents — any of them. The design goal: a
developer on Claude Code, opencode, Pi, Oh My Pi, Codex CLI, or a harness that didn't
exist when this was written gets the same instructions, the same procedures, and the same
quality gate. [`AGENTS.md`](../AGENTS.md) is the entry point for all of it.

## The three layers

| Layer | Where | Loads | Rule |
|---|---|---|---|
| Invariants | [`AGENTS.md`](../AGENTS.md) | every session, at launch | keep it under ~200 lines — a new invariant earns a line, nothing more |
| Procedures | [`.claude/skills/`](../.claude/skills)/*/SKILL.md | on demand, when the situation matches | one skill per procedure; plain markdown |
| Gates | [`scripts/hooks/`](../scripts/hooks/README.md) | at the "about to claim done" moment | one exit-code contract; wiring is per-harness |

The split is a context budget, not a filing system: what loads every session must stay
small, so it carries only invariants; procedures live in files an agent reads when
relevant; gates are scripts that need no context at all.

`CLAUDE.md` is deliberately a stub — one `@AGENTS.md` import plus a Claude-specific note —
because Claude Code reads `CLAUDE.md`, not `AGENTS.md`, and every other harness reads
`AGENTS.md` directly. One source of truth, two front doors.

## How each harness consumes it

| Harness | Instructions | Skills | Stop gate |
|---|---|---|---|
| Claude Code | `CLAUDE.md` → imports AGENTS.md | native — discovers `.claude/skills/` (dir name = the slash command) | wired — tracked [`.claude/settings.json`](../.claude/settings.json) Stop hook |
| opencode | reads AGENTS.md | native — discovers `.claude/skills/` at project level (walks up to the worktree root) | run [`scripts/hooks/stop-check.mjs`](../scripts/hooks/stop-check.mjs) at your done-moment, honor exit 2 |
| Pi | reads AGENTS.md (cwd + parents) | via the skills index in AGENTS.md — read the file when the row matches | same |
| Oh My Pi | reads AGENTS.md; inherits `.claude`/`.codex`-style workspace config | via the index | same |
| Codex CLI | reads AGENTS.md | via the index | same |
| anything else | AGENTS.md if it reads any instruction file | via the index | the git pre-push recipe in [`scripts/hooks/README.md`](../scripts/hooks/README.md) is universal |

The per-harness rows are verified against each tool's current documentation and — for the
workbench harnesses — against the harness registry in `api/src/workbench/harnesses.rs`.
When a harness changes its discovery rules, this table changes with it.

## The skills format

Every skill is `.claude/skills/<name>/SKILL.md` with YAML frontmatter whose first `---` is
line one of the file:

- `name` — required by opencode, must match the directory (`^[a-z0-9]+(-[a-z0-9]+)*$`).
  Claude Code doesn't require it, so the strictest consumer sets the rule.
- `description` — required; this is what every harness shows when deciding whether the
  skill is relevant. Write the trigger into it ("Use when…").

Bodies are plain markdown in the repo's voice — any agent that can read a file can follow
one. Note what these are **not**: [`scripts/skills/`](../scripts/skills) is product
surface, shipped into Hermes agent containers by `api/src/agent_skills.rs` — repo tooling
never goes there.

## The stop gate

[`scripts/hooks/stop-check.mjs`](../scripts/hooks/stop-check.mjs) runs `bun run check` —
the same chain CI runs — under the exit-code contract in
[`scripts/hooks/README.md`](../scripts/hooks/README.md) (exit 0 pass-silent, exit 2 block
with the reason on stderr, anything else a non-blocking error). It always runs on the
whole tree, no diff fast path: the check is seconds and dependency-free, and scope-skipping
is a second place for checker scope to rot. Because parallel sessions share working trees,
a gate failure may predate your change — the block message says what to do about that.

## What is deliberately not here

- **No tracked permissions.** `.claude/settings.json` carries only the Stop hook;
  permission allowlists are personal (`settings.local.json`, untracked — the tracked
  `.gitignore` covers it, along with `.claude/worktrees/`).
- **No repo MCP client config.** The MCP server in `mcp/` is product (agents in a
  Talaria workspace); pointing a harness at a dev instance needs a running org and is a
  later tranche.
- **`.claude/**` is not doc-walked** by `check-docs` (it skips dotted directories), so
  links inside skills are unenforced — keep them few and pointed at stable paths. The
  index rows in AGENTS.md are enforced, which is where it counts.
