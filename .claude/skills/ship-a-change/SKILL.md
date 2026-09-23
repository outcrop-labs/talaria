---
name: ship-a-change
description: Land a change — the pre-push gates, exercising the changed path in the running app, the CHANGELOG entry, commit conventions, the pull request against rc (never main), and the post-PR watcher. Use when a change is code-complete and ready to verify, commit, or PR, or when an open PR is red or conflicted and must be fixed before claiming done.
---

# Ship a change

The outer loop: from code-complete to merged. The change lands on `rc` — the integration
branch and the staging environment — and `main` receives it later, by an automated promotion
once the staging deploy is green. You never open that second pull request. The model, and why:
[`docs/BRANCHES.md`(../../../docs/BRANCHES.md). The rules in full are CONTRIBUTING.md's; this is
the procedure in order.

Opening the pull request is not the end of the loop. Do not report the work done while its
checks are failing, still pending, or the PR conflicts with `rc` — that is section 6.

## 1. Gates

```bash
bun run gate         # local pre-push: check, then only the compiles this diff touches
```

- `gate` runs `check` always. It runs typecheck, tests, clippy and `cargo test` only for the surfaces and packages the diff touches, and it never runs a workspace cargo. That is the point: `api:check`, `desktop:check`, `verify`, and `cargo test` / `clippy` without `-p` lock `api/target` and pin the machine for every other agent in the checkout.
- Do not run those "to be safe", and do not set `CARGO_BUILD_JOBS` or pass `--jobs` to lift the cap in `api/.cargo/config.toml`. CI runs the full surface job. A red CI log names the crate; reproduce with `cargo test -p <that crate>` (and clippy the same way) — one package, never the workspace.
- A check failure you believe is a false positive gets argued in the PR, never silenced by widening a pattern or exempting a path. The invariant scripts encode real incidents; widening one to pass is how the next incident ships.

## 2. Exercise the path you changed

Typecheck alone doesn't prove a surface works (CONTRIBUTING rule 2). `bun talaria dev` →
drive the actual surface at <http://localhost:5273>:

- data-dependent surfaces: `node scripts/seed-daily-brief.mjs` first (empty installs show
  nothing — see the dev-loop skill);
- API paths: mint a session (repo-traps skill, trap 5) and curl for real;
- UI: the browser; screenshots are evidence.
- Changed how the image boots, or anything the container's own entrypoint touches? Prove it
  the way CI does: `docker build -t talaria-rc:local . && node scripts/deploy-smoke.mjs
  --image talaria-rc:local` — a fresh instance, and the assertions a deploy makes.

## 3. CHANGELOG

Add `changelog/YYYY-MM-DD-<slug>.md` containing the entry verbatim: **what changed**, in a
bold lead sentence, and **what you verified** — the gate you ran and how you exercised the
path. One file per entry — two open PRs never collide at the `[Unreleased]` anchor, and
`bun scripts/changelog-roll.mjs --check` (part of `bun run check`) fails a hand-appended
bullet. The changelog is the record reviewers and release-notes readers actually have;
"verified: typecheck" on a behavior change is a red flag you should catch yourself.

## 4. Commit

- Style: `area: lowercase sentence — explanation` — read `git log` for the voice; the
  em-dash clause says why, not what.
- **Stage by explicit path.** Parallel sessions share working trees: `git status` first,
  then `git add <files>`. Never `git add -A`, never `git clean`, never an unscoped reset —
  uncommitted files may be another session's work in progress.
- One change per commit; the changelog entry file rides with the change it describes.
- Push the branch you are on. `main` and `rc` are not push targets: `talaria setup` wires
  `scripts/hooks/pre-push` in, and it refuses those pushes locally (CI refuses them too).

## 5. PR — against `rc`

- **Base `rc`.** Never `main`: a pull request aimed at the trunk fails `flow.yml`'s
  `pr-target` check, and nobody reviews there — review, CI and the staging deploy all happen
  on `rc`.
- Body: what changed, and what was verified (mirroring the CHANGELOG entry).
- If the stop gate or `check` failed on a pre-existing whole-tree failure, say so in the
  PR — another session's mid-edit file, verified, not silently absorbed.
- If a check match was a false positive, the argument lives in the PR where a reviewer
  can veto it.

CI runs the full `bun run verify` suite on the pull request, and the push to `rc` that
follows runs `rc-deploy.yml` — which builds this commit's image, boots it and checks it. That
deploy, not this pull request, is what lets the promotion be offered (`rc → main`); a
maintainer merges it, and the evidence is on it when it appears — nobody has to assemble it
by hand.

## 6. After the PR opens — watch, then fix, then claim done

Before you claim done, remove what this task created. The convention and the command are
the [cleanup](../cleanup/SKILL.md) skill. The stop gate runs the sweep and blocks if disk
pressure or stale artifacts are over the line — forgetting is how the disk fills.

The stop gate and the pre-push hook saw the local tree. CI runs the full tree, and `rc`
can move under the branch while the PR waits (a sibling merge is the usual way it
conflicts). Start the watcher before you claim completion, and cite only a state it
printed. Never a check or a mergeable bit you did not read back.

```bash
node scripts/hooks/pr-watch.mjs
```

Same exit contract as [`scripts/hooks/README.md`](../../../scripts/hooks/README.md). The
script is [`scripts/hooks/pr-watch.mjs`](../../../scripts/hooks/pr-watch.mjs). It polls
check runs and mergeable state (first poll immediate, then 20s exponential backoff
capped at 2min, hard stop at 45 minutes) and does not edit, merge, push, or comment.

| Exit | First line | What you do |
|---|---|---|
| 0 | `pr-watch: green` | Checks passed and GitHub reports the PR mergeable (no conflict with `rc`). You may claim done. Cite the proof line. A required review is not this gate — exit 0 does not mean a human approved, and you still do not merge. |
| 2 | `pr-watch: red` | Read the log tail and the job URL. Diagnose. Fix. Re-run `bun run gate`. If the log names a crate this diff did not touch, `cargo test -p <that crate>` (clippy the same way) — never `api:check`, never a workspace cargo. Push to this same branch. Run the watcher again. |
| 2 | `pr-watch: conflict` | `git fetch origin rc && git merge origin/rc`. Resolve. Re-run `bun run gate`. Push to this same branch. Run the watcher again. |
| 2 | `pr-watch: timeout` | Still pending when the wall clock ran out. Not done, and not a pass. Report the PR URL. Do not claim done. |
| 2 | `pr-watch: pending` | Only with `--once`. Not done. Re-run without `--once` to wait. |
| 1 | `pr-watch: could not run` | Not a pass. Say so. Do not claim done. |

A stderr line that starts `pr-watch: pending —` is progress, not a decision. The decision
is the exit code and a first line with no em-dash.

Self-fix pushes go to the branch you are on — an `agent/*` branch, never `main`, never
`rc`, never `--force`. `scripts/hooks/pre-push` refuses the long-lived branches anyway.
A re-push is visible on the PR timeline and does not ping reviewers. Whether it should
is an open human call; the default is silent, and this script does not comment.

Stop and report the failing job link instead of claiming done when it is not self-fixable:

- the same check is still red after a push that was supposed to fix it
- the failure is a product decision, a review request, or infra you cannot fix from this
  branch (runner, registry, a missing secret)
- the conflict is not an honest textual resolution — you would be choosing product
  behavior, or regenerating a file you do not know how to regenerate

Whether `finish_job` should refuse a done claim whose PR is red or conflicted is a
platform question. Not this procedure. Flag it; do not build it here.