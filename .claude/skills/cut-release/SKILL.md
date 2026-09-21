---
name: cut-release
description: Cut a release — tag an RC on rc or a stable release on main, and know what the release workflow publishes to GHCR. Use when cutting an RC or stable release, re-running a nightly, or diagnosing why a channel or image tag did not move.
---

# Cut a release

RELEASING.md is the contract — this is the working procedure distilled from it. The two facts
that shape everything: **git tags are the version authority** (releasing never edits a file —
no version-bump commit, no changelog commit, nothing to merge afterwards), and **a release is
cut from a branch that has already been deployed**: `rc` for a candidate, `main` for a stable.
The branch model behind that: [`docs/BRANCHES.md`(../../../docs/BRANCHES.md).

## The branches

| Branch | Role | How it moves | Tag cut on it |
|---|---|---|---|
| `rc` | integration + staging; every PR targets it | merging pull requests | `vX.Y.Z-rc.N` |
| `main` | the release trunk | the promotion merge of `rc`, once its deploy is green | `vX.Y.Z` |
| `testing` | retired — nightly builds `rc`'s tip now | nothing; delete it | — |

## Cut an RC

```bash
git switch rc && git pull                # the tip you mean to cut
git tag v0.2.0-rc.1 && git push origin v0.2.0-rc.1
```

Tag the tip whose staging deploy you watched (Actions → rc-deploy); that deploy is what an RC
is supposed to carry. Nothing needs advancing first — `rc` is where the work is.

The tag push runs the release workflow: full CI against the tag, images land on GHCR as
`0.2.0-rc.1` + (moving) `rc`, and a GitHub **prerelease** opens with a stub body pointing
at CHANGELOG.md — edit the notes after if you want more, or leave the stub.

**Tag grammar is exact:** `vX.Y.Z-rc.N`, lowercase, exactly. `v1.0`, `v1.0.0-beta.1`,
`v1.0.0-RC.1` all fail the workflow loudly rather than publishing. A misfire is deleted
by deleting the tag.

## Promote to stable

Two steps, in this order: the promotion, then the tag.

```bash
# 1. main catches up — promote.yml verifies rc's tip once its deploy is green and
#    offers the rc → main PR (opened for you when a PROMOTION_TOKEN is set).
#    Merge it as a merge commit. Actions → promote if it has not appeared.
# 2. tag what main now is:
git switch main && git pull
git tag v0.2.0 && git push origin v0.2.0
```

Publishes `0.2.0` + (moving) `latest` and a regular GitHub Release. `latest` moves on
nothing else — only a suffix-free `vX.Y.Z` tag. A stable desktop release also attaches
`latest.json` and `.sig`; that needs the `TAURI_SIGNING_PRIVATE_KEY` Actions secret. An RC
does not publish `latest.json`.

## Nightlies

Every day 03:17 UTC, the workflow builds `rc`'s tip: tags `nightly` (moving) and
`nightly-YYYYMMDD` (frozen, kept). No GitHub Release — 365 prereleases is noise; the dated
image tags carry the history. Re-run one from Actions → release → Run workflow. **If
nightlies just stop:** GitHub disables schedules after 60 days of repo inactivity — that
is the first thing to check; the second is a `startup_failure`, which produces no jobs and
no logs (RELEASING.md has the story).

## The images

| Tag | Moves when | Immutable? |
|---|---|---|
| `nightly` / `rc` / `latest` / `main` | their build, tag or promotion | no — pointers |
| `nightly-YYYYMMDD` / `X.Y.Z-rc.N` / `X.Y.Z` | their build or tag | yes |

Everything is `ghcr.io/outcrop-labs/talaria` (the app) and `ghcr.io/outcrop-labs/talaria-api`
(the api package — same tags **plus** an immutable `sha-<sha12>` per commit; that sha tag
is what a release's app-image build is pinned to, and what `rc-deploy.yml` pins for staging).

Production instances run from a separately operated infrastructure deployment — this repo
ends at the published images. The changelog is already maintained by PRs; releasing adds
no notes work.