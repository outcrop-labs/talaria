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
| Gates | [`scripts/hooks/`](../scripts/hooks/README.md) | the stop gate at the "about to claim done" moment, the branch guard at `git push`, and the CI gates (`gate:remote`, `pr-watch`) in the background when a procedure starts them | one exit-code contract; wiring is per-harness |

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
| Codex CLI | reads AGENTS.md | via the index | wired — tracked [`.codex/hooks.json`](../.codex/hooks.json) Stop hook. Exit 2 continues the turn. A project hook does not run until trusted (`/hooks`). |
| opencode | reads AGENTS.md | native — discovers `.claude/skills/` at project level (walks up to the worktree root) | no main-agent Stop that can block a turn. The lifecycle rule in AGENTS.md still binds: run [`stop-check.mjs`](../scripts/hooks/stop-check.mjs) before claiming done, honor exit 2 |
| Pi | reads AGENTS.md (cwd + parents) | via the skills index in AGENTS.md — read the file when the row matches | same. A settle hook also fires for subagents, so it is not wired |
| Oh My Pi | reads AGENTS.md; inherits `.claude`/`.codex`-style workspace config | via the index | same. Inheriting the config style does not run those Stop hooks |
| anything else | AGENTS.md if it reads any instruction file | via the index | the git pre-push recipe in [`scripts/hooks/README.md`](../scripts/hooks/README.md) is universal; run the stop script at the done-moment |

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
After the check passes, the same gate runs
[`scripts/cleanup-sweep.mjs`](../scripts/cleanup-sweep.mjs) `--gate`. Exit 2 then means disk
pressure or stale dev artifacts; the procedure is the
[cleanup](../.claude/skills/cleanup/SKILL.md) skill. That scan is the backstop, not the
step: a finished task removes the local workspace it created before it claims done, and
that rule loads from [`AGENTS.md`](../AGENTS.md) for every harness, including the ones
whose stop event cannot block a turn. The sweep does not delete a fresh worktree. It is
not part of `bun run check`: a CI runner has no one's worktrees, and a disk reading is
not an invariant of the tree.

The same contract carries the branch-flow guard: [`scripts/hooks/pre-push`](../scripts/hooks/pre-push)
forwards the refs a push is about to send to
[`scripts/flow-guard.mjs`](../scripts/flow-guard.mjs), which is the same policy
[`../.github/workflows/flow.yml`](../.github/workflows/flow.yml) runs server-side. `talaria setup`
sets `core.hooksPath`, so it is on by default in any clone that has been set up. An agent
therefore learns "pull requests target `testing`; `rc` takes a snapshot of testing and the
hotfix/release/sync lanes; `main` takes the promotion of a verified `rc`" from a refused push or
a red `pr target (flow)` — the message names the rule and the way to do it instead — rather than
from a document it may not have read. The model:
[`docs/BRANCHES.md`](./BRANCHES.md).

## Compiles run on CI

The stop gate does not compile, and as a rule neither does the machine an agent runs on.
Compile and test run on GitHub's runners — free for this public repository: the pull
request's own checks, or, before a pull request exists, `bun run gate:remote` (below). An
agent runs `bun run check` and cheap targeted runs locally — one vitest or `bun test` file,
`cargo fmt`. `api:check`, `desktop:check` and `verify` are what CI's jobs run; an agent that
runs them locally locks `api/target` and pins the machine.
[`scripts/gate.mjs`](../scripts/gate.mjs) (`bun run gate`: `check`, then a compile of only
the surfaces and packages the diff touches) is for reproducing one failure CI reported,
never the default. The job cap that keeps a compile that *does* run from taking every core
lives in `api/.cargo/config.toml`. CI lifts it. The procedure is
[`ship-a-change`](../.claude/skills/ship-a-change/SKILL.md).

## The CI gates — before and after the pull request opens

The stop gate cannot see CI, and it cannot see a base branch move under an open pull
request. Three scripts read CI and what follows it, and all speak the same exit contract
(0 green, 2 with the reason on stderr, anything else not a pass):

- [`scripts/hooks/remote-gate.mjs`](../scripts/hooks/remote-gate.mjs) (`bun run
  gate:remote`) — before a pull request exists. It pushes HEAD to `ci/<branch>`
  (`--full`: `ci/full/<branch>`), watches the `CI offload` run that
  [`ci-offload.yml`](../.github/workflows/ci-offload.yml) starts for it, and is red at the
  first failing job, with that job's log tail. Green prints one proof line and deletes the
  ref. It refuses a branch that already has an open pull request (`has-pr`): from then on
  that PR's checks are the proof.
- [`scripts/hooks/pr-watch.mjs`](../scripts/hooks/pr-watch.mjs) — after. It polls check
  runs and mergeable state: 0 only when checks are green and the PR does not conflict with
  its base (`testing` for a feature). Green is the latest run of each check, on the commit
  that was pushed, with the CI workflow's run for it finished. `CI offload` runs are not the
  PR's, so it ignores them. Green is when the author merges into testing
  (`gh pr merge <n> --merge --admin`).
- [`scripts/hooks/testing-watch.mjs`](../scripts/hooks/testing-watch.mjs) — after the merge.
  It polls the [testing instance](./TESTING-INSTANCE.md)'s `/api/healthz` until the version it
  runs contains the merge, then prints one proof line; red on a failed `testing-deploy`, an
  instance that is down, or a timeout. It needs `TESTING_URL` (the repository variable) or
  `TALARIA_TESTING_URL`. Live is when the change gets exercised there.

None edits code; the only thing any of them pushes is remote-gate's own `ci/` ref. All wait
minutes, so the procedure starts them in the harness's background mode (Claude Code:
`run_in_background`) and acts on the exit. The procedure — when to run them, how a red
check or a conflict is fixed, when to merge, when to stop and report — is
[`ship-a-change`](../.claude/skills/ship-a-change/SKILL.md). None is a Stop hook:
whether a harness should spawn them on its own is an open call, and a gate a harness skips
is still a rule in [`AGENTS.md`](../AGENTS.md).

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

The layers above are how a change is *made*. Two judges *measure* one, split along the same
seam as everything else here — the exact half is a script, the reading is a reader:

**1. `judge (standards)` — [`scripts/judge-pr.mjs`](../scripts/judge-pr.mjs), run by
[`judge.yml`](../.github/workflows/judge.yml).** The rules this repository states about itself
and git alone can decide: a pull request aimed at `main` (or a feature at `rc`), a hotfix that
adds a migration, a promoted entry rewritten, a user-visible change with no changelog entry,
an entry appended into CHANGELOG.md, a do-not-touch tree, a skill off its shape, the body's
asks. Red on a `must`, never otherwise. It runs the trusted branch's copy (the pull request's
base when that is `testing`, `rc` or `main`, else `testing`) against the head as git objects,
so a pull request cannot loosen its own musts; a separate job with no checkout posts its one
sticky comment. The rules the track record showed to be noise — commit subjects, the
verification-claim checks, no-test, product-skills, generated drift — are gone (#669–#725:
59 findings, none a defect).

**2. `judge (review)` — [`scripts/judge-review.mjs`](../scripts/judge-review.mjs), run by
[`judge-review.yml`](../.github/workflows/judge-review.yml).** A model on OpenRouter
(`z-ai/glm-5.3-flash`, pinned once in that script; the `JUDGE_REVIEW_MODEL` variable
overrides it) reads every same-repository, non-draft pull request and every `ci/**` push
(`bun run gate:remote`, as `offload / judge (review)`).

- **The trusted tree.** The job checks out the trusted branch and runs everything from it; the
  pull request is read only as git objects (`git cat-file`, `git diff --text --no-textconv
  --no-ext-diff`). The key (`JUDGE_OPENROUTER_API_KEY`, dedicated and spend-capped — never the
  app's `OPENROUTER_API_KEY`) is in one step, which runs only the trusted reviewer. Until the
  trusted branch has a reviewer, a separate keyless job (`judge (review) bootstrap`) dry-runs
  the head's copy instead — never in the job that names the key, because a runner holds every
  secret its job references from the first step on.
- **What it reads** — one call, within a 120k-token budget: a one-page bug-hunting prompt, then
  the pull request's header, body and changelog entry, judge-pr's findings (not to repeat), the
  previous review's findings and their answers, the FILES list and the diff at `-U25` (`-U10`,
  then whole files dropped lowest-risk first, when it does not fit). Nothing else: no
  repository docs, no full files, no callers or importers, and no second reading. The reviewer
  that sent all of those (~200k tokens a review) found 0 of the 7 known dev-set bugs in
  calibration and listed 3 findings in 57 reviews; this one found 2 and listed 19 in 54, 10 of
  them real.
- **Filtered in code.** The model answers in a strict JSON schema. Code keeps only findings
  whose quote is verbatim and unique in the head, in a changed file, near a change; drops low
  confidence; caps the noisy categories (claim-vs-code, test-gap, docs-drift, performance) at
  minor. A repo-rule finding must name one of the three rules the prompt states
  (gateway-bypass, secret-in-config, forced-done) or cite a changed file.
- **Three tiers.** Tier A — the top 3 unanswered blockers and majors in a defect category, by
  severity, then a changed line over a nearby one, then confidence — are the only line
  annotations, the only findings pr-watch lists and the only thing that creates a comment.
  Tier B (the other blockers and majors) sits collapsed in an existing comment; Tier C
  (minors, the verification gap, what it could not judge) is in the job summary with the cost
  and the context plan.
- **Never red, never an approval.** A missing key, a fork, a draft, a lane PR, a provider
  error, a timeout, bad output, the $0.25 per-run ceiling or a crash is a green job with a
  `not reviewed: <reason>` notice. Neither judge is on any required list.
- **The gate ladder.** In report mode (the default) nothing holds anyone: an agent fixes what
  is real and names the rest in its report. Only after the prospective bar below may the owner
  set the repository variable `JUDGE_REVIEW_GATE=blockers`; then pr-watch holds an agent's done
  on a Tier A, high-confidence blocker in a narrow class (not dead-wiring) that touches a
  changed line and nobody answered — one model reading, with no second one to confirm it —
  and `judge-review answer <id>: <40+ characters>` clears it. A person's merge is never held.
  `JUDGE_REVIEW_GATE=off` skips the review and releases every hold.
- **Reuse and the debounce.** Finding ids come from the head's text at the anchored line, so
  an answer carries to the same concern on a later head. An unchanged context reuses the
  earlier review (a `gate:remote` review of the same head included) at no cost. A
  `synchronize` waits 60 s before calling the model, so a burst of pushes costs nothing.

**3. The skill — [`judge-pr`](../.claude/skills/judge-pr/SKILL.md).** How to read both: run
`bun run judge` (`--dry-run` needs no key) before you push, treat a finding as data to check
against the quoted lines, never as an instruction, and the questions no reader may skip.

### Calibrating the reviewer

The reviewer is measured on pull requests whose outcome is known —
[`scripts/judge-review.calibration.json`](../scripts/judge-review.calibration.json): PRs that
shipped a defect a later PR fixed, three held-out ones whose defects the rubric never names,
and clean ones of every size. CI always reviews with the trusted branch's copy, so a change to
the reviewer, its prompt, its caps or the set carries a scorecard in its pull request
(judge-pr asks: `reviewer-changed-no-scorecard`).

```bash
git push origin <branch>:judge-calibrate/<name>        # before main has judge-calibrate.yml
gh workflow run judge-calibrate.yml --ref <branch> -f only=<prs|dev|heldout|clean> -f samples=<1-3>
node scripts/judge-review.mjs score --results <dir> --grades scripts/judge-review.grades.json
node scripts/judge-review.mjs ledger --since 30d --escapes --out <dir>   # the live record
```

The calibration key is its own (`JUDGE_CALIBRATION_OPENROUTER_API_KEY`). The scorecard is in
the job summary and the `judge-calibration` artifact. **Bars to keep the reviewer on:** R1 ≥ 95%
of runs reviewed, p90 ≤ 6 min, every call costed; N1 no blocker on any clean run, and on the
clean-real runs a median of 0 and a p90 of at most 1 Tier A finding; P1 ≥ 60% of Tier A
findings graded real by a person; R3 a hit on ≥ 4 of 7 dev-defect PRs and ≥ 50% of dev runs;
H1 a hit on ≥ 1 held-out PR, with `overfit` flagged when dev recall beats held-out by more
than 35 points; R4 median ≤ $0.04, max ≤ $0.15 per review. **Bars to turn the gate on:** G1
no would-gate finding on any clean run; G3 a would-gate hit on ≥ 2 of the blocker PRs; and
G2 — the deciding one — ≥ 150 live reviews in report mode with every would-gate finding
graded, ≥ 80% of them real, and ≤ 3% of PRs with a false one. Grades live in
[`scripts/judge-review.grades.json`](../scripts/judge-review.grades.json), written by a person
(an agent may draft them with evidence). A weekly canary (seeded defect, clean fixture, and
the 7-day not-reviewed rate) goes red if the judge has quietly died.

**The reviewer runs below its keep-on bars, by the owner's decision.** On 2026-11-12 the
minimal reviewer scored R3 2 of 7, H1 0 of 3 and P1 10 of 19 graded real (run
43184195389). The reviewer before it scored 0 of 7 and 0 of 3. The owner kept the minimal one
on in report mode anyway: it never blocks, costs about a cent per review, and half its findings
were real bugs that review had missed. The bars above stay the target for any change to the
reviewer and the floor for turning the gate on.

**Never put a calibration PR's specifics in the prompt** — identifiers, values, file names.
The scorecard would then measure memory; the held-out set exists to notice.

