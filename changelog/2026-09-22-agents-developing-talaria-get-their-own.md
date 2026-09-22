- **Agents developing Talaria get their own floor.** `AGENTS.md` is the
  canonical instruction file for anyone — human or agent, on any harness —
  coding in this repo: the distilled rules, the command map, environment
  facts (ports, worktree isolation), parallel-session etiquette, the skills
  index, and the do-not-touch list, kept under ~200 lines because it loads
  into agent context at launch; `CLAUDE.md` is a stub importing it, since
  Claude Code reads `CLAUDE.md` while opencode, Pi, Oh My Pi, and Codex read
  `AGENTS.md` directly. Four skills (`.claude/skills/`: dev-loop, repo-traps,
  ship-a-change, cut-release) carry the procedures — frontmatter `name` +
  `description` so Claude Code and opencode both discover them natively, with
  the AGENTS.md index as the path for everything else — including the
  operational traps that cost real time, each phrased symptom → check → fix
  and verified against the code they describe (the session-mint field order
  against `api/src/session.rs`, the scheduler kill switch against
  `api/src/scheduler.rs`). The stop gate is a harness-agnostic library:
  `scripts/hooks/stop-check.mjs` runs `bun run check` (seconds, zero
  dependencies) under one contract — exit 0 silent pass, exit 2 block with
  the reason on stderr, anything else a non-blocking error — wired into
  Claude Code by a tracked `.claude/settings.json` holding only the Stop
  hook; permissions stay personal in the untracked `settings.local.json`,
  which the tracked `.gitignore` now covers along with `.claude/worktrees/`.
  How the layers fit and how each harness consumes them is documented at
  `docs/AGENT-TOOLING.md`. Also corrects the stale "frozen at cutover" notes
  about `docs/api/` in CONTRIBUTING.md and DEVELOPERS.md — the #293 extractor
  landed; the reference is generated. Verified: `bun run check` green from a
  checkout with no `node_modules` (every new link resolves, no generated
  drift), the hook standalone passes silent and exits 2 on a planted dead doc
  link, `git check-ignore` proves both new ignores, and Claude Code loads the
  stub and lists the four skills (`/context`, `/skills`).
