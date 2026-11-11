---
name: cut-release
description: Cut a release — tag an RC on rc or a stable release on main, and know what the release workflow publishes to GHCR. Use when cutting an RC or stable release, re-running a nightly, or diagnosing why a channel or image tag did not move.
---

# Cut a release

RELEASING.md is the contract — this is the working procedure distilled from it. The two facts
that shape everything: **git tags are the version authority** (no file carries the version —
no version-bump commit; the one file a release changes is CHANGELOG.md, through the roll's
pull request below), and **a release is cut from a branch that has already been deployed**:
`rc` for a candidate, `main` for a stable.
The branch model behind that: [`docs/BRANCHES.md`](../../../docs/BRANCHES.md).

## The branches

| Branch | Role | How it moves | Tag cut on it |
|---|---|---|---|
| `rc` | integration + staging; every PR targets it | merging pull requests | `vX.Y.Z-rc.N` |
| `main` | the release trunk | the promotion merge of `rc`, once its deploy is green | `vX.Y.Z` |
| `testing` | retired — nightly builds `rc`'s tip now | nothing; delete it | — |

## Cut an RC

```bash
# 1. the roll, as a pull request against rc — like every other change
git fetch origin && git switch -c release/roll-0.2.0-rc.1 origin/rc
bun scripts/changelog-roll.mjs 0.2.0-rc.1      # folds changelog/*.md into a section, deletes them
git add CHANGELOG.md changelog/
git commit -m "changelog: roll 0.2.0-rc.1 — the entries since the last release"
git push -u origin release/roll-0.2.0-rc.1
gh pr create --base rc --title "changelog: roll 0.2.0-rc.1"
# 2. merge it as a merge commit once CI is green; watch rc-deploy deploy that merge
# 3. tag the merge
git switch rc && git pull
grep -n '^## 0.2.0-rc.1' CHANGELOG.md            # the section is in the tree you tag
git tag v0.2.0-rc.1 && git push origin v0.2.0-rc.1
```

Roll the changelog BEFORE tagging — the tagged tree must carry the section, so the tag says
what it holds and the promotion carries it to main. (The release's notes link is
`blob/main/CHANGELOG.md`, not the tag: an RC's section shows there only after rc is
promoted.) **Never push the roll commit to rc
yourself:** `scripts/flow-guard.mjs` refuses an rc tip that is not a merge (the pre-push
hook locally, `flow.yml` in CI), so the roll lands the way everything does — a pull request
against rc. Hold other merges to rc between the roll's merge and the tag: a PR that lands in
between ships in the tag with its entry still in `changelog/`, outside the section. The roll
refuses to run on an empty `changelog/`; if it does, that is a prompt to read the entries,
not a step to force.

Tag the tip whose staging deploy you watched (Actions → rc-deploy); that deploy is what an RC
is supposed to carry. Nothing else needs advancing first — `rc` is where the work is.

The tag push runs the release workflow: full CI against the tag, images land on GHCR as
`0.2.0-rc.1` + (moving) `rc`, and a GitHub **prerelease** opens with a stub body pointing
at CHANGELOG.md — edit the notes after if you want more, or leave the stub.

**Tag grammar is exact:** `vX.Y.Z-rc.N`, lowercase, exactly. `v1.0`, `v1.0.0-beta.1`,
`v1.0.0-RC.1` all fail the workflow loudly rather than publishing. A misfire is deleted
by deleting the tag.

## Promote to stable

**CI runs the chain** (RELEASING.md, "Promoting to stable"):

1. On a verified rc with `changelog/` entries, `promote.yml` opens the roll's pull request
   from `release/roll`. `scripts/release-version.mjs` picks the version: the next patch, an
   entry's `<!-- bump: minor -->`, or a `version` dispatch.
2. Once that merges and rc verifies, it opens `promote: rc → main — vX.Y.Z`.
3. Its merge makes `tag-release.yml` tag main and dispatch `release.yml` at the tag. That
   publishes the images and a Release whose notes come from `scripts/release-notes.mjs`.

The roll and promotion pull requests need `PROMOTION_TOKEN`; without it the run summary
prints the commands. The by-hand steps below are the fallback, and the only path for a
stable whose `changelog/` was empty (nothing pending, so nothing for `tag-release.yml` to
tag).

Two steps, in this order: the promotion, then the tag.

```bash
# 1. main catches up — promote.yml verifies rc's tip once its deploy is green and
#    offers the rc → main PR (opened for you when a PROMOTION_TOKEN is set).
#    Merge it as a merge commit. Actions → promote if it has not appeared.
# 2. tag what main now is:
git switch main && git pull
git tag v0.2.0 && git push origin v0.2.0
```

A stable's notes come from the same roll. If entries landed on rc after the last RC's roll,
roll them the same way — a pull request against rc — before step 1, under the stable's own
version (`bun scripts/changelog-roll.mjs 0.2.0`, a `## 0.2.0` section above the RC ones). If
the roll refuses because `changelog/` is empty (the usual case right after an RC), nothing
landed since the RC's roll and the RC sections already cover the stable: skip the roll and
go to step 1. A roll can never go to main directly: main takes rc's content and nothing else
(flow-guard).

Publishes `0.2.0` + (moving) `latest` and a regular GitHub Release. `latest`, the image
tag, moves on nothing else — only a suffix-free `vX.Y.Z` tag.

**The GitHub Release is never "Latest"** — that flag is the desktop updater feed. The shell
reads `/releases/latest/download/latest.json`, which must resolve to the newest `desktop-v*`
release (desktop-package.yml marks each mint Latest once its `latest.json` is up). A stable
`v*` release would take the flag by default, it carries no `latest.json`, and every
update check would 404, so `release.yml` creates and edits it `--latest=false` and
`check-invariants.mjs` (`release-latest-is-the-desktop-feed`) fails a workflow that drops a
flag. Do not mark it Latest in the Releases page either.

**No server tag builds or attaches desktop installers** — `vX.Y.Z` and `vX.Y.Z-rc.N` alike.
Talaria Desktop ships only as `desktop-vX.Y.Z` releases, which `desktop-package.yml` mints on
a main push that touches `desktop/` (or a `publish=true` dispatch). Desktop has its own
version line: a 0.13.0 Desktop built from a server tag beside `desktop-v0.12.0` would never be
offered an update until Desktop passed 0.13. A stable `v*` cut still raises the desktop
auto-minor's baseline (RELEASING.md, "The desktop installers").

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
ends at the published images. The changelog is already maintained by PRs (one entry file
each); releasing adds exactly one step: the roll above, which is a pull request like any other.