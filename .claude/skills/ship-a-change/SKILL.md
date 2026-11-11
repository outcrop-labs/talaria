---
name: ship-a-change
description: Land a change — the gates (CI is the compile, never this machine), exercising the changed path, the CHANGELOG entry, commit conventions, the pull request against testing (never rc or main), the post-PR watcher, the merge into testing at green, and testing-watch until it is live. Use when a change is code-complete and ready to verify, commit, or PR, when an open PR is red or conflicted and must be fixed before claiming done, or when a green PR is ready to merge into testing.
---

# Ship a change

The outer loop: from code-complete to merged, live and exercised. The change lands on
`testing` — the integration lane, where every merge deploys the testing instance — and you
merge it yourself once its checks are green. `rc` takes it later in a snapshot of `testing`,
and `main` after that by promotion; you never open those pull requests. The model, and why:
[`docs/BRANCHES.md`](../../../docs/BRANCHES.md). The rules in full are CONTRIBUTING.md's; this is
the procedure in order.

Opening the pull request is not the end of the loop. Do not report the work done while its
checks are failing, still pending, or the PR conflicts with its base — that is section 6 —
nor before it is merged, live on testing, and exercised there — section 7.

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
- No local api build: `bun talaria dev --api-from-ci` runs the api binary CI built for
  this commit (or the nearest ancestor that changed the api).
- Changed how the image boots, or anything the container's own entrypoint touches? CI
  builds and smokes the image: `testing-deploy.yml` builds the merged commit's image, boots
  a fresh instance and runs `scripts/deploy-smoke.mjs` against it (`rc-deploy.yml` does the
  same on rc). Build locally only when debugging the Dockerfile itself.

Before the PR this is the local stack. After the merge, the same path is exercised again on
the testing instance — section 7.

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
- Push the branch you are on. `main`, `rc` and `testing` are not push targets: `talaria
  setup` wires `scripts/hooks/pre-push` in, and it refuses those pushes locally (CI refuses
  them too).

## 5. PR — against `testing`

- **Base `testing`.** `gh pr create --base testing`. Never `rc` or `main`: `flow.yml`'s
  `pr-target` refuses a feature into either (`rc` takes only snapshots of testing and the
  hotfix/release/sync lanes), and judge-pr grades it a `must`. Start the branch from
  `origin/testing` (`talaria worktree <name> origin/testing`) so the diff is only yours.
- Body: what changed, and what was verified (mirroring the CHANGELOG entry), with the run
  links — the PR's checks, a `gate:remote` proof line.
- If the stop gate or `check` failed on a pre-existing whole-tree failure, say so in the
  PR — another session's mid-edit file, verified, not silently absorbed.
- If a check match was a false positive, the argument lives in the PR where a reviewer
  can veto it.
- `bun run gate:remote` also reviews the branch (`offload / judge (review)`, the model
  reading — [judge-pr](../judge-pr/SKILL.md) §2). Read what it prints; fix what is real.
- A body edit re-runs `judge (standards)` only. A retarget makes `judge (review)` stale:
  `gh run rerun <id>` (pr-watch prints it) reviews against the new base.

CI comes in three weights, and one mode for the promotion, all planned by
`scripts/surfaces.mjs`. **Light** — Linux jobs for the surfaces the branch moved — runs on
a `ci/<branch>` offload and a pull request into `testing`. **Integration** — the Linux jobs
for everything since testing's last green run — runs on the push to `testing` your merge
makes, beside `testing-deploy.yml`. **Full** — the surfaces the change moved, each with its
macOS, Windows, iOS and e2e legs — runs on a pull request into `rc` (the snapshot, a hotfix)
and a push to `rc` or `main`; the daily battery of testing and `ci/full/<branch>`
(`gate:remote --full`) run every surface that way. A platform break your light run could
not see turns up there, and comes back to you as a fix into testing.
The `rc → main` pull request runs in **promotion** mode — `what changed` and the
invariants only — because its tree is rc's tip, which rc's push CI and `rc-deploy` already
verified. A pull request that touches the app image's build context also builds the image
(`app image builds (docker)`), so a broken `.dockerignore` fails before the merge.

**A hotfix** is the one change that goes to `rc` directly: a fix rc needs before the next
snapshot. Cut it from `origin/rc` (`git switch -c hotfix/<name> origin/rc`), never from
testing — `pr-target` refuses a head carrying testing's commits — and add no migration
(the `migrations` job and judge-pr both refuse one; the array index is a migration's
identity, and testing's array is ahead). Open it against `rc`; full CI runs, a person reviews
and merges it, and the back-flow into testing is opened automatically.

## 6. After the PR opens — watch, then fix

Do not tear the local workspace down while the watcher is still red — you may need it
to fix, and you may need it again if the change breaks on testing. Once it is green,
merged and live (section 7), remove what this task created before you claim done. The
convention and the command are the [cleanup](../cleanup/SKILL.md) skill. The stop gate
blocks if disk pressure or stale artifacts are over the line; that is the backstop, not
the step.

The stop gate and the pre-push hook saw the local tree. CI runs the full tree, and
`testing` can move under the branch while the PR waits (a sibling merge is the usual way
it conflicts). Start the watcher in the background as soon as the PR is open — it can wait
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
| 0 | `pr-watch: green` | Checks passed and GitHub reports the PR mergeable (no conflict with its base). Cite the proof line. Into `testing`: merge it and watch it go live (section 7). Into `rc` or `main`: a person reviews and merges — exit 0 does not mean a human approved, and you do not merge. The proof line may list findings to read (`judge (review)`, confirmed by a second reading). Fix the real ones; name the others in your report with one sentence each. No PR comment is needed. |
| 2 | `pr-watch: review` | Only when JUDGE_REVIEW_GATE=blockers: a doubly confirmed blocker nobody answered. Read it against the code. Real → fix, push, re-watch. Wrong → `gh pr comment <n> --body 'judge-review answer <id>: <evidence>'`, re-watch. Still holding after one fix attempt → answer it, or stop and report to a person. |
| 2 | `pr-watch: red` | Read the log tail and the job URL. Diagnose. Reproduce only that failure locally, if you must: `cargo test -p <crate> <test>` (clippy the same way), `bunx vitest run <file>` — never `api:check`, never a workspace cargo. Fix, `bun run check`, commit, push to this same branch; re-run the watcher (it is in the background). On the `rc` → `main` promotion, the fix is a `hotfix/` branch from `origin/rc` and a pull request against `rc`; on a snapshot into `rc`, a pull request into `testing`, then a fresh snapshot. |
| 2 | `pr-watch: conflict` | `git fetch origin <base> && git merge origin/<base>` — `testing` for a feature, `rc` for a hotfix. Resolve. `bun run check`, commit, push to this same branch; re-run the watcher (it is in the background). On a promotion nobody pushes the head: a snapshot is replaced by a fresh one once the back-flow merges; for `rc` → `main`, bring `main` into `rc` through a `sync/` pull request against `rc`, as the body says. |
| 2 | `pr-watch: timeout` | Still pending when the wall clock ran out. Not done, and not a pass. Report the PR URL. Do not claim done. |
| 2 | `pr-watch: pending` | With `--once`, or at once when waiting cannot change the head (not pushed, or dropped from the PR). Not done. Do what the reason says (push, or reconcile with `origin/<branch>`), or re-run without `--once` to wait. |
| 1 | `pr-watch: could not run` | Not a pass. Say so. Do not claim done. |

A stderr line that starts `pr-watch: pending —` is progress, not a decision. The decision
is the exit code and a first line with no em-dash.

Self-fix pushes go to the branch you are on — an `agent/*` branch, never `main`, `rc` or
`testing`, never `--force`. `scripts/hooks/pre-push` refuses the long-lived branches anyway.
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

## 7. Merge and see it live

A pull request into `testing` is yours to merge once the watcher is green, and the change is
done when it runs on the testing instance and you have exercised it there.

1. **pr-watch green → merge.** `gh pr merge <n> --merge --admin` — a merge commit, never a
   squash. `--admin` bypasses the review rule (`testing-review`), never the checks
   (`testing-checks` binds admins): a merge while a required check is pending or red is
   refused, which is the point. Without admin rights — the Talaria App, an outside
   contributor — the merge waits for an approving review: report the green PR as
   "not ready to merge: waiting on review", and stop there.
2. **Start the deploy watch in the background:**
   `node scripts/hooks/testing-watch.mjs <n>` (Claude Code: `run_in_background`). Merge to
   live is about 10–15 minutes, 20–30 when the api changed. Exit 0 is one proof line —
   `testing-watch: live — <url> runs testing-<sha12>, which contains <target12>`; cite it.
   Exit 2 names why waiting will not help: `red-deploy` (open the run — a broken image is
   yours to fix with a new pull request into testing), `down`, `unmerged`, `timeout`.
3. **Exercise the changed path on the testing instance** — the same surface as section 2,
   on a real deploy with real agents. API paths use
   [`docs/TESTING-INSTANCE.md`](../../../docs/TESTING-INSTANCE.md) "Agent access" (the
   repo-traps trap-5 session over `ssh -i ~/.ssh/talaria-testing`, as the non-admin test
   user). While it cannot run — no `TESTING_URL` yet, so testing-watch could not run — say so,
   and exercise locally with `bun talaria dev --api-from-ci` on the merge commit instead.
4. **Then claim done** — after the cleanup skill has removed this task's workspace, citing
   the pr-watch and testing-watch proof lines and what you exercised. Something broken on
   testing is a new pull request into testing, never a push to it.