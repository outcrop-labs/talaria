# Gate hooks

The repo's local quality gates as a library: the check logic lives in shared scripts, the
wiring is per-harness, and the contract below is the only thing a harness needs to know.
Nothing here requires an install — every script is stdlib-only and runs in a fresh
worktree before `bun install` has ever happened.

## The contract

Every hook in this directory speaks it:

| Signal | Meaning |
|---|---|
| stdin | the caller's event payload, whatever shape — a hook reads it if it needs it (`pre-push` does: git hands it the refs), and one that needs nothing tolerates it |
| exit 0 | pass — silent |
| exit 2 | block — the reason is on stderr |
| any other exit | the gate could not run — bad usage, an unresolvable ref; never a pass |

A Stop gate and a git hook read that last row differently, and the difference is worth
knowing: **git refuses a push on any non-zero hook exit**, so exit 1 from
[`pre-push`](./pre-push) stops the push and names what to fix; a harness running a Stop gate
decides for itself what exit 1 means. Neither ever treats "could not run" as a pass — that
is the part that matters.

Five entry points:

- [`stop-check.mjs`](./stop-check.mjs) — runs `bun run check` (invariants + doc links +
  generated-reference drift) and then [`../cleanup-sweep.mjs`](../cleanup-sweep.mjs) `--gate`
  (disk pressure and stale dev artifacts). Both under this contract, on the whole tree, with
  no fast path — see its header for why the check has no skip list, and why the sweep is not
  part of `bun run check`.
- [`pre-push`](./pre-push) — the branch-flow guard. Git invokes it with one line per ref about
  to leave the machine (`<local ref> <local sha> <remote ref> <remote sha>`) and forwards each
  to [`../flow-guard.mjs`](../flow-guard.mjs), the same policy
  [`../../.github/workflows/flow.yml`](../../.github/workflows/flow.yml) runs server-side. It
  judges `main`, `rc` and `testing`: a push to any of them that the model does not allow —
  a direct commit, a squash, a rewrite, a merge of testing's content into rc that is not its
  tree — stops here with the reason; the model is
  [`../../docs/BRANCHES.md`](../../docs/BRANCHES.md).
- [`pr-watch.mjs`](./pr-watch.mjs) — after a pull request is open. Polls check runs and
  mergeable state. Exit 0 only when checks are green (the latest run of each, per event; a
  superseded cancelled one does not count), the PR's head is the pushed commit, a CI workflow
  run for that head has finished, and the PR is mergeable against its base — `testing` or
  `rc` for a feature, by lane (`laneOf`: the `promote/*` → `rc` and `rc` → `main` promotions,
  the `backflow/*` → `testing` back-flow, the `hotfix/`, `release/` and `sync/` lanes into
  `rc`). A promotion is watched the same way; its head is read on GitHub, and its red and
  conflict advice is a pull request into the branch it came from, never a push. A
  `bun run gate:remote` run of the same sha (`ci/…` heads, the `CI offload` workflow) is not
  the PR's evidence and is ignored; a red the base branch's own push CI already has is
  still `red`, labelled not self-fixable. The CI workflow's name comes
  from the PR's own repository (the one gh's PR URL names, else `--repo`): its `ci.yml` at
  the PR head, read from GitHub. Only when GitHub cannot answer does this checkout's copy (if it is
  that repository) or `CI` stand in, and the verdict says so. Every `gh` read names the PR's
  host (`--hostname` on `gh api`, `HOST/OWNER/REPO` on `--repo`), so a GitHub Enterprise PR,
  or a `GH_HOST` set for another host, does not send it elsewhere;
  exit 2 names `red`, `conflict`, `pending`, `timeout`, `closed`, or `base` (a base in no
  lane) on stderr
  (`pending` comes at once, not at the wall clock, when only a push or a reconcile can
  change the head). It does not edit or push. Not wired as a Stop hook — [`ship-a-change`](../../.claude/skills/ship-a-change/SKILL.md)
  runs it, because whether a harness should spawn it on its own is an open call. Exit 0
  prints one proof line to stdout (the state an agent may cite); every other rule of this
  contract holds, including "could not run is not a pass".
- [`remote-gate.mjs`](./remote-gate.mjs) — `bun run gate:remote`: CI before a pull request
  exists. It refuses a tree with tracked changes, `main`/`rc`/`testing`, and a branch that
  already has an open PR (that PR's CI is the proof; `pr-watch.mjs` reads it). Then it runs
  `bun run check` and `cargo fmt --check` (rustfmt, no compile) on the touched crates,
  force-pushes HEAD to `ci/<branch>` (`--full`, or a `hotfix/`, `release/`, `sync/`,
  `promote/` or `backflow/` branch: `ci/full/<branch>`), and watches the new `CI offload` run
  ([`../../.github/workflows/ci-offload.yml`](../../.github/workflows/ci-offload.yml)). It is
  red the moment any job fails, with that job's log tail, while the rest still run. Exit 0
  prints one proof line naming the run, and deletes the ref. Exit 2's verdict opens with
  the token line: `remote-gate: dirty`, `base`, `has-pr`, `check`, `fmt`, `no-run`, `red`,
  `cancelled`, `timeout` or `pending` (`--detach`). Anything on stderr before it is
  progress (always `remote-gate: <word> — …`), `bun run check`'s output, or `bun run`'s own
  `$ node …` echo. A red, cancelled or timed-out ref is kept, for
  `--rerun` (re-run the failed jobs and watch the new attempt; refused once the ref holds
  another commit, whose run a re-run would cancel) and `--watch <run-id>` (resume). It talks to GitHub over REST with a token — `GH_TOKEN`, `GITHUB_TOKEN`, or `gh
  auth token` — so a devbox needs no gh (`talaria box new <name> --env GH_TOKEN=…`).
  Procedure-invoked, not a hook: it runs for minutes, so run it in the harness's background
  mode and act on the exit. Nothing it leaves lasts: ci-offload's daily prune deletes `ci/**`
  refs older than 24 hours, and the cleanup sweep deletes a merged worktree's.
- [`testing-watch.mjs`](./testing-watch.mjs) — after a pull request merges into `testing`:
  is it live on the [testing instance](../../docs/TESTING-INSTANCE.md) yet? It polls the
  instance's `/api/healthz` until the version it reports (`testing-<sha12>`) is a commit that
  contains the merge (local git: `merge-base --is-ancestor`, so run it from a clone). Exit 0
  prints one proof line, `testing-watch: live — …`. Exit 2's first stderr line is the token:
  `testing-watch: unmerged`, `red-deploy` (a testing-deploy run carrying the commit failed and
  nothing newer is building), `down` (healthz failing for over 5 minutes) or `timeout`. It
  needs a GitHub token (`GH_TOKEN`, `GITHUB_TOKEN`, or `gh auth token`) and the instance's
  URL — `TALARIA_TESTING_URL`, else the repository variable `TESTING_URL`; with neither it
  could not run, never a pass. Procedure-invoked like the two above: run it in the
  background after the merge, then exercise the change there.

## Wiring it

| Harness | How |
|---|---|
| Claude Code | already wired — the tracked [`../../.claude/settings.json`](../../.claude/settings.json) Stop hook |
| Codex CLI | already wired — the tracked [`../../.codex/hooks.json`](../../.codex/hooks.json) Stop hook. Exit 2 continues the turn with stderr as the next prompt. A project hook does not run until it is trusted (`/hooks`). |
| Pi, Oh My Pi, opencode | no main-agent Stop event that can block a turn. A settle hook also fires for subagents, so it is not wired — the full check on every subagent is not a lifecycle. The rule in [`AGENTS.md`](../../AGENTS.md) still binds: run [`stop-check.mjs`](./stop-check.mjs) at the done-moment and honor exit 2. |
| git (works under any harness) | `talaria setup` sets `core.hooksPath` to this directory, which is what makes [`pre-push`](./pre-push) run; per-clone by hand: `git config core.hooksPath scripts/hooks` |
| CI | nothing to do — [`../../.github/workflows/ci.yml`](../../.github/workflows/ci.yml) runs `bun run check` itself, and [`../../.github/workflows/flow.yml`](../../.github/workflows/flow.yml) runs the branch policy. CI has no one's worktrees. |
| Before a PR exists, any harness | `bun run gate:remote` ([`remote-gate.mjs`](./remote-gate.mjs)) in the background (Claude Code: `run_in_background`); honor exit 2. Not a Stop hook: a run takes minutes. Needs a GitHub token (gh's login, or `GH_TOKEN` in a devbox). |
| After the merge into testing, any harness | `node scripts/hooks/testing-watch.mjs <n>` ([`testing-watch.mjs`](./testing-watch.mjs)) in the background; honor exit 2. Not a Stop hook: merge to live is minutes. Needs a GitHub token and `TESTING_URL` (or `TALARIA_TESTING_URL`). |
| anything else | run [`stop-check.mjs`](./stop-check.mjs) before you claim done, [`pr-watch.mjs`](./pr-watch.mjs) after the PR opens, and [`testing-watch.mjs`](./testing-watch.mjs) after the merge; honor exit 2. Removing the local workspace is still your step — the sweep does not delete a fresh one. |

A git hook is bypassable (`git push --no-verify`) and a stop gate is not a wall either; both
are there so the right thing is the easy thing. What makes the branch model hold is the
server-side half — the rulesets, and `flow.yml` failing the push after the fact.

## Adding a hook

A new dependency-free script in this directory, speaking the same contract; wire it once per
harness. Keep it runnable before any install — a gate that needs `node_modules` is a gate a
fresh worktree silently skips. A file git has to find by name (`pre-push`, `pre-commit`) needs
that exact name and the executable bit, not a `.mjs` extension.
