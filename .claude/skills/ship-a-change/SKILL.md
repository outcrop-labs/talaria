---
name: ship-a-change
description: Land a change — the pre-push gates, exercising the changed path in the running app, the CHANGELOG entry, commit and PR conventions, and staging hygiene when parallel sessions share the tree. Use when a change is code-complete and ready to verify, commit, or PR.
---

# Ship a change

The outer loop: from code-complete to merged. The rules in full are CONTRIBUTING.md's;
this is the procedure in order.

## 1. Gates

```bash
bun run check        # fast inner gate — seconds, no install needed
bun run verify       # the PR gate: check + typecheck + test — green before every push
```

- Touched `api/`? `bun run verify` **and** `bun run api:check` (fmt + clippy
  `-D warnings` + cargo tests — the CI api job).
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

## 5. PR

- Body: what changed, and what was verified (mirroring the CHANGELOG entry).
- If the stop gate or `check` failed on a pre-existing whole-tree failure, say so in the
  PR — another session's mid-edit file, verified, not silently absorbed.
- If a check match was a false positive, the argument lives in the PR where a reviewer
  can veto it.

CI runs the full `bun run verify` suite on the PR; green locally first means green there
on the first try.
