---
name: ship-a-change
description: Land a change — the gates (CI is the compile, never this machine), exercising the changed path in the running app, the CHANGELOG entry, commit conventions, the pull request against rc (never main), and the post-PR watcher. Use when a change is code-complete and ready to verify, commit, or PR, or when an open PR is red or conflicted and must be fixed before claiming done.
---

# Ship a change

The outer loop: from code-complete to merged. The change lands on `rc` — the integration
branch and the staging environment — and `main` receives it later, by an automated promotion
once the staging deploy is green. You never open that second pull request. The model, and why:
[`docs/BRANCHES.md`](../../../docs/BRANCHES.md). The rules in full are CONTRIBUTING.md's; this is
the procedure in order.

Opening the pull request is not the end of the loop. Do not report the work done while its
checks are failing, still pending, or the PR conflicts with `rc` — that is section 6.

## 1. Gates — CI is the compile

Compile and test run on GitHub's runners (free for this public repository), not on this
machine. Locally you run only what is cheap.

```bash
bun run check                    # local: invariants, doc links, generated drift — seconds, no install
node scripts/hooks/pr-watch.mjs  # the PR's own CI — in the background (section 6)
bun run gate:remote              # before a PR exists: CI on ci/<branch> — in the background
```

1. **`bun run check` locally**, plus cheap targeted runs of what you touched: one
   `bunx vitest run <file>`, one `bun test <file>`, `cargo fmt` on the crate.
2. **Code-complete → open the pull request** (section 5; `gh pr create --draft` is fine).
   Its own CI is the proof. Start `node scripts/hooks/pr-watch.mjs` in the harness's
   background mode (Claude Code: `run_in_background`) and act on its exit (section 6). Mark
   the PR ready (`gh pr ready`) once it is green.
3. **Not ready for a PR → `bun run gate:remote`**, also in the background. It needs a
   committed tree, pushes HEAD to `ci/<branch>`, watches that `CI offload` run, and is red at
   the first failing job with its log tail — iterate on that. `--full` adds macOS, Windows,
   iOS and e2e. Exit 0 is one proof line (cite it); exit 2 opens with a token line (`red`,
   `dirty`, `check`, `fmt`, …). A killed harness loses nothing: `--watch <run-id>` resumes,
   and a red run keeps its ref for `--rerun`. It refuses once a PR exists (`has-pr`) — from
   then on the PR's checks are the proof.
4. **Only to reproduce one failure CI reported:** `cargo test -p <crate> <test>` (clippy the
   same way), `bunx vitest run <file>`. `bun run gate` is the local scoped compile for that
   case, never the default.

- Never a workspace cargo: `api:check`, `desktop:check`, `verify`, and `cargo test` /
  `clippy` without `-p` lock `api/target` and pin the machine for every other agent in the
  checkout. Do not run them "to be safe", and do not set `CARGO_BUILD_JOBS` or pass `--jobs`
  to lift the cap in `api/.cargo/config.toml`.
- A check failure you believe is a false positive gets argued in the PR, never silenced by widening a pattern or exempting a path. The invariant scripts encode real incidents; widening one to pass is how the next incident ships.

## 2. Exercise the path you changed

Typecheck alone doesn't prove a surface works (CONTRIBUTING rule 2). `bun talaria dev` →
drive the actual surface at <http://localhost:5273>:

- data-dependent surfaces: `node scripts/seed-daily-brief.mjs` first (empty installs show
  nothing — see the dev-loop skill);
- API paths: mint a session (repo-traps skill, trap 5) and curl for real;
- UI: the browser; screenshots are evidence.
- Changed how the image boots, or anything the container's own entrypoint touches? CI
  builds and smokes the image: `rc-deploy.yml` builds the merged commit's image, boots a
  fresh instance and runs `scripts/deploy-smoke.mjs` against it (after the cutover,
  testing-deploy's smoke does the same). Build locally only when debugging the Dockerfile
  itself.

## 3. CHANGELOG

Add `changelog/YYYY-MM-DD-<slug>.md` containing the entry verbatim: **what changed**, in a
bold lead sentence, and **what you verified** — the gate that proved it ("CI: this PR's
checks", or `gate:remote`) and how you exercised the path. Never a run URL: a commit that
only adds a link re-runs the whole battery. The URL goes in the PR body
(`gh pr edit <n> --body-file …`), which re-runs only the judge (and `flow`, on a promotion).
One file per entry — two open PRs never collide at the `[Unreleased]` anchor, and
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
- Body: what changed, and what was verified (mirroring the CHANGELOG entry), with the run
  links — the PR's checks, a `gate:remote` proof line.
- If the stop gate or `check` failed on a pre-existing whole-tree failure, say so in the
  PR — another session's mid-edit file, verified, not silently absorbed.
- If a check match was a false positive, the argument lives in the PR where a reviewer
  can veto it.

CI comes in two weights, planned by `scripts/surfaces.mjs`. **Light** — Linux jobs for the
surfaces the branch moved — runs on a `ci/<branch>` offload and a pull request into anything
but `rc` or `main`. **Full** — every surface, plus macOS, Windows, iOS and e2e — runs on a
pull request into `rc` or `main`, a push to either, and `ci/full/<branch>`
(`gate:remote --full`). The push to `rc` that the merge makes runs `rc-deploy.yml` — which
builds this commit's image, boots it and checks it. That
deploy, not this pull request, is what lets the promotion be offered (`rc → main`); a
maintainer merges it, and the evidence is on it when it appears — nobody has to assemble it
by hand.

## 6. After the PR opens — watch, then fix, then claim done

Do not tear the local workspace down while the watcher is still red — you may need it
to fix. Once the proof line is green, remove what this task created before you claim
done. The convention and the command are the [cleanup](../cleanup/SKILL.md) skill. The
stop gate blocks if disk pressure or stale artifacts are over the line; that is the
backstop, not the step.

The stop gate and the pre-push hook saw the local tree. CI runs the full tree, and `rc`
can move under the branch while the PR waits (a sibling merge is the usual way it
conflicts). Start the watcher in the background as soon as the PR is open — it can wait
45 minutes, and a harness kills a foreground command long before that — and cite only a
state it printed. Never a check or a mergeable bit you did not read back.

```bash
node scripts/hooks/pr-watch.mjs   # background mode (Claude Code: run_in_background); act on the exit
```

Same exit contract as [`scripts/hooks/README.md`](../../../scripts/hooks/README.md). The
script is [`scripts/hooks/pr-watch.mjs`](../../../scripts/hooks/pr-watch.mjs). It polls
check runs and mergeable state (first poll immediate, then 20s exponential backoff
capped at 2min, hard stop at 45 minutes) and does not edit, merge, push, or comment.
It reads the latest run of each check, so a judge run that a body edit cancelled and
re-ran is not red. Green also needs the PR's head to be the commit you pushed and a
finished CI workflow run for that head, so running it straight after a push waits
instead of reading the previous head, or a rollup that holds only `judge`. The CI
workflow's name is read from the PR's repository at its head, so `--repo owner/name`
(or a PR URL) watches another repository's PR from any checkout; a name it had to
guess is named as a guess in the verdict. A PR
head that contains your HEAD (someone pushed on top) passes, even when this clone
has not fetched it. Two head states cannot be waited out, so the watcher exits
`pending` at once instead of polling: a HEAD you have not pushed, and a PR head
that no longer contains the HEAD you pushed (the branch moved on GitHub without it).

| Exit | First line | What you do |
|---|---|---|
| 0 | `pr-watch: green` | Checks passed and GitHub reports the PR mergeable (no conflict with `rc`). You may claim done only after the local workspace this task created is gone (cleanup skill). Cite the proof line. A required review is not this gate — exit 0 does not mean a human approved, and you still do not merge. |
| 2 | `pr-watch: red` | Read the log tail and the job URL. Diagnose. Reproduce only that failure locally, if you must: `cargo test -p <crate> <test>` (clippy the same way), `bunx vitest run <file>` — never `api:check`, never a workspace cargo. Fix, `bun run check`, commit, push to this same branch; re-run the watcher (it is in the background). On the promotion, the fix is its own branch from `origin/rc` and a pull request against `rc`. |
| 2 | `pr-watch: conflict` | `git fetch origin rc && git merge origin/rc`. Resolve. `bun run check`, commit, push to this same branch; re-run the watcher (it is in the background). On the promotion (`rc` → `main`) nobody pushes `rc`: bring `main` into `rc` through a pull request against `rc`, as the body says. |
| 2 | `pr-watch: timeout` | Still pending when the wall clock ran out. Not done, and not a pass. Report the PR URL. Do not claim done. |
| 2 | `pr-watch: pending` | With `--once`, or at once when waiting cannot change the head (not pushed, or dropped from the PR). Not done. Do what the reason says (push, or reconcile with `origin/<branch>`), or re-run without `--once` to wait. |
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