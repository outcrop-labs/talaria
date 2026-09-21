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
| Gates | [`scripts/hooks/`](../scripts/hooks/README.md) | at the "about to claim done" moment — and, for the branch guard, at `git push` | one exit-code contract; wiring is per-harness |

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

The same contract carries the branch-flow guard: [`scripts/hooks/pre-push`](../scripts/hooks/pre-push)
forwards the refs a push is about to send to
[`scripts/flow-guard.mjs`](../scripts/flow-guard.mjs), which is the same policy
[`../.github/workflows/flow.yml`](../.github/workflows/flow.yml) runs server-side. `talaria setup`
sets `core.hooksPath`, so it is on by default in any clone that has been set up. An agent
therefore learns "pull requests target `rc`, and `main` takes the promotion of a verified `rc`"
from a refused push — the message names the rule and the way to do it instead — rather than from
a document it may not have read. The model:
[`docs/BRANCHES.md`](../docs/BRANCHES.md).

## What is deliberately not here

- **No tracked permissions.** `.claude/settings.json` carries only the Stop hook;
  permission allowlists are personal (`settings.local.json`, untracked — the tracked
  `.gitignore` covers it, along with `.claude/worktrees/`).
- **No repo MCP client config.** The MCP server in `mcp/` is product (agents in a
  Talaria workspace); pointing a harness at a dev instance needs a running org and is a
  later tranche.
- **`.claude/skills/` IS doc-walked, and nothing else under `.claude/` is** (`check-docs` skips
  dotted directories except `.github` and `.claude`, and skips `.claude/worktrees/`, which holds
  whole checkouts). The links inside a skill are read by every agent that follows one, so they
  are checked like any other doc — a wrong relative path there used to be invisible for exactly
  as long as nobody read the skill. Keep them pointed at stable paths: a doc that moves takes
  every skill link with it, and `bun run check` will say so. The index rows in AGENTS.md ARE
  enforced, and in both directions, by the `skills-index-drift` rule in
  [`scripts/check-invariants.mjs`](../scripts/check-invariants.mjs): a skill with no row, a
  directory with no `SKILL.md`, and a row pointing at nothing all fail `bun run check`.

## The judge

The layers above are how a change is *made*.
[`judge-pr`](../.claude/skills/judge-pr/SKILL.md) is how one is *measured*, split along the same
seam as everything else here:

- [`scripts/judge-pr.mjs`](../scripts/judge-pr.mjs) is the mechanical half — the diff-level
  things the repo states about itself: a pull request aimed at `main`, a user-visible change
  with no changelog entry, a verification claim that misses the surface it covers, a
  hand-edited generated reference, a commit subject off the house style, a do-not-touch tree.
  It prints the lines it judged, and exits 2 on a `must` like every other gate here.
- the [skill](../.claude/skills/judge-pr/SKILL.md) is the other half: reading the change against
  the application, each question pointed at the doc that decides it. No script can tell a
  working fallback from a dormant one, and a judge that guesses is worse than none.
- [`.github/workflows/judge.yml`](../.github/workflows/judge.yml) runs the script on every pull
  request, keeps one sticky comment carrying the verdict, and fails on a `must` — deliberately
  not a required check until its findings earn that, the same promotion path every other check
  here took.
