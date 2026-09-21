# Pull request

**Base branch: `rc`.** Every PR targets `rc` — the integration and staging branch. `main`
moves only through the promotion flow: a PR offering `rc` to `main`, raised by
`promote.yml` once the staging deploy is verified and merged by a maintainer. Never open a PR
against `main`; one that does gets retargeted, and the branch-flow guard
([`scripts/flow-guard.mjs`](../scripts/flow-guard.mjs), run by CI) fails until it is.

## Checklist

- [ ] This PR targets `rc`.
- [ ] `bun run verify` is green locally, and `bun run api:check` too if `api/` changed.
- [ ] I exercised the changed path in the running app; the steps are under **Verified**.
- [ ] [`CHANGELOG.md`](../CHANGELOG.md) carries what changed and what was verified.
- [ ] Generated files were regenerated, not hand-edited —
      [`docs/CLI-REFERENCE.md`](../docs/CLI-REFERENCE.md) and
      [`docs/api/README.md`](../docs/api/README.md).

## What changed

<!-- The behaviour, not the diff: what was wrong or missing, and what it does now. -->

## Verified

<!-- The gate that ran, and how the changed path was exercised in the running app. -->

- **Gate:**
- **Exercised:**

The gates run on the whole tree, so a failure can predate your branch. That failure is not
yours to fix here, but it is yours to name: reproduce it on a clean `rc` checkout, quote that
under **Gate**, and say so. An absorbed failure hides a broken tree from every PR after this
one ([`AGENTS.md`](../AGENTS.md) — "Parallel sessions share working trees").

## Branch flow

`scripts/flow-guard.mjs` (run by CI) holds the branch contract. Does this PR touch it, the
promotion flow, or the workflows that carry them?

<!-- Yes: what moved and why. No: "no". -->

## CHANGELOG entry

<!-- Paste the entry, or say why the change is internal-only. -->