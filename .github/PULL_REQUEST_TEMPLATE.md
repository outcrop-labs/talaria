# Pull request

**Base branch: `testing`** (`rc` only for `promote/`, `hotfix/`, `release/`, `sync/` heads).
`testing` is the integration lane: light CI, the author merges at green, and every merge
deploys the testing instance. `rc` takes a frozen snapshot of `testing` plus the hotfix, roll
and sync lanes cut from `rc`; `main` moves only through the promotion of `rc`. A feature
opened against `rc` or `main` gets retargeted, and the branch-flow guard
([`scripts/flow-guard.mjs`](../scripts/flow-guard.mjs), run by CI) fails until it is.

## Checklist

- [ ] This PR targets `testing` (or `rc`, from a `promote/`, `hotfix/`, `release/` or `sync/` head).
- [ ] `bun run check` is green locally; this PR's CI is the compile/test proof (links to runs or a `gate:remote` result go under **Verified**, never in the entry file).
- [ ] I exercised the changed path in the running app; the steps are under **Verified**.
- [ ] A `changelog/YYYY-MM-DD-<slug>.md` file ([CONTRIBUTING.md](../CONTRIBUTING.md), step 3) carries what changed and
      what was verified. [`CHANGELOG.md`](../CHANGELOG.md) is written only by the release roll — never by hand.
- [ ] Generated files were regenerated, not hand-edited —
      [`docs/CLI-REFERENCE.md`](../docs/CLI-REFERENCE.md) and
      [`docs/api/README.md`](../docs/api/README.md).

## What changed

<!-- The behaviour, not the diff: what was wrong or missing, and what it does now. -->

## Verified

<!-- The gate that ran — this PR's CI, or `bun run gate:remote` before it opened, with the run links —
     and how the changed path was exercised in the running app. Editing this body starts no compile. -->

- **Gate:**
- **Exercised:**

`bun run check` (which CI and `gate:remote` both run) sees the whole tree, so a failure can predate your branch. That failure is not
yours to fix here, but it is yours to name: reproduce it on a clean checkout of the base, quote that
under **Gate**, and say so. An absorbed failure hides a broken tree from every PR after this
one ([`AGENTS.md`](../AGENTS.md) — "Parallel sessions share working trees").

Two judges comment. `judge (standards)` holds the exact rules; a `must` is red. `judge (review)`
is a model's reading; it is never red. Before you push, run `bun run judge` (needs
`JUDGE_OPENROUTER_API_KEY`), or `bun run gate:remote` to have your branch reviewed on GitHub. If
you change the reviewer, paste the scorecard from a `judge-calibrate/<name>` push. How to read
both: [`.claude/skills/judge-pr/SKILL.md`](../.claude/skills/judge-pr/SKILL.md).

## Branch flow

`scripts/flow-guard.mjs` (run by CI) holds the branch contract. Does this PR touch it, the
promotion flow, or the workflows that carry them?

<!-- Yes: what moved and why. No: "no". -->

## CHANGELOG entry

<!-- Paste the entry from your changelog/ file, or say why the change is internal-only. -->