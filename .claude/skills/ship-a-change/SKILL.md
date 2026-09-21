---
name: ship-a-change
description: Land a change — the pre-push gates, exercising the changed path in the running app, the CHANGELOG entry, commit conventions, and the pull request against rc (never main). Use when a change is code-complete and ready to verify, commit, or PR.
---

# Ship a change

The outer loop: from code-complete to merged. The change lands on `rc` — the integration
branch and the staging environment — and `main` receives it later, by an automated promotion
once the staging deploy is green. You never open that second pull request. The model, and why:
[`docs/BRANCHES.md`(../../../docs/BRANCHES.md). The rules in full are CONTRIBUTING.md's; this is
the procedure in order.

## 1. Gates

```bash
bun run check        # fast inner gate — seconds, no install needed
bun run verify       # the PR gate: check + typecheck + test — green before every push
```

- Touched `api/`? `bun run verify` **and** `bun run api:check` (fmt + clippy
  `-D warnings` + cargo tests — the CI api job). Touched `desktop/`? `bun run desktop:check`.
- A check failure you believe is a false positive gets argued in the PR, never silenced by
  widening a pattern or exempting a path. The invariant scripts encode real incidents;
  widening one to pass is how the next incident ships.

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

Append to the `[Unreleased]` section of `CHANGELOG.md`: **what changed**, in a bold lead
sentence, and **what you verified** — the gate you ran and how you exercised the path.
The changelog is the record reviewers and release-notes readers actually have; "verified:
typecheck" on a behavior change is a red flag you should catch yourself.

## 4. Commit

- Style: `area: lowercase sentence — explanation` — read `git log` for the voice; the
  em-dash clause says why, not what.
- **Stage by explicit path.** Parallel sessions share working trees: `git status` first,
  then `git add <files>`. Never `git add -A`, never `git clean`, never an unscoped reset —
  uncommitted files may be another session's work in progress.
- One change per commit; the CHANGELOG entry rides with the change it describes.
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