# Releasing Talaria

How the channels work, how a release is cut, and what the automation does.
The operator-facing half (running a prebuilt image) is
[`docs/CONTAINER.md` → Prebuilt images](./docs/CONTAINER.md); the machinery
lives in [`.github/workflows/release.yml`](./.github/workflows/release.yml).

## The model

**Git tags are the version authority.** Every `package.json` in this repo
says `"0.1.0"` decoratively and nothing reads them; the running version of
any install is the commit it runs (the Updates panel shows it), and the
version of any published image is the tag that built it. Releasing never
edits a file — no version-bump commit, no changelog commit, nothing to
merge afterwards.

Two long-lived branches, both already deployed before a version is cut:
[`docs/BRANCHES.md`](./docs/BRANCHES.md) is the model in full.

| Branch | Role | How it moves | What a tag on it means |
|---|---|---|---|
| `rc` | the integration branch, and the staging environment: every pull request targets it, and `rc-deploy.yml` deploys its tip | merging pull requests (merge commits) | `vX.Y.Z-rc.N` — a release candidate of what `rc` currently is |
| `main` | the release trunk — what customers run | only the promotion merge of `rc` into it, after `rc` is deployed and verified | `vX.Y.Z` — a stable release of what `main` currently is |

There is no third branch to advance and no fast-forward to remember: `rc` is
where the work is, `main` is where it lands once the staging deploy says it
works, and `promote.yml` does that landing by itself. What remains a human's
call is the one thing that should be: whether a version exists.

## Cutting an RC

```bash
git switch rc && git pull                     # the tip you mean to cut
git tag v0.2.0-rc.1 && git push origin v0.2.0-rc.1
```

Nothing needs advancing first: `rc` is where the work already is, and its tip
has been deployed and smoke-tested by `rc-deploy.yml` on the way in — which is
the guarantee an RC is supposed to carry. Tag the tip whose deploy you watched
(`git log --oneline -1` against the run list under Actions → rc-deploy).

The tag push runs the release workflow: the full CI suite against the tag,
then the image builds and lands on GHCR as `0.2.0-rc.1` and (moving) `rc`,
the desktop installers are built for linux, macOS and Windows and attached to
the release, and a GitHub **prerelease** opens with a stub body. The changelog is the
record — edit the release notes afterwards if you want them to say more,
or leave the stub pointing at CHANGELOG.md.

A malformed tag (`v1.0`, `v1.0.0-beta.1`, `v1.0.0-RC.1` — the grammar is
`vX.Y.Z-rc.N`, lowercase, exactly) fails the workflow loudly rather than
publishing anything. Delete a misfire by deleting the tag.

One trap when you re-cut a tag that already opened a release: **GitHub converts
that release to a draft** when its tag goes away, and the next run finds the
draft, refreshes its notes and uploads its assets into it — so the release looks
done to anyone with push access and is invisible to everyone else. Publish it
again once the re-run is green:

```bash
gh release edit v0.2.0-rc.1 --draft=false
```

## Promoting to stable

An RC becoming stable is two steps, in this order: the promotion, then the tag.

```bash
# 1. main catches up: promote.yml verifies rc's tip once its staging deploy is
#    green and offers the rc → main pull request (it opens it for you when a
#    PROMOTION_TOKEN is configured). Merge it as a merge commit — the one merge
#    that carries a whole verified branch.
# 2. tag what main now is:
git switch main && git pull
git tag v0.2.0 && git push origin v0.2.0
```

Publishes `0.2.0` and (moving) `latest`, and a regular GitHub Release.
`latest` moves on nothing else — only a `vX.Y.Z` tag with no suffix.

A stable desktop release also attaches `latest.json` and `.sig` files so
installed copies can update in-app. That needs the
`TAURI_SIGNING_PRIVATE_KEY` Actions secret (the matching pubkey is in
`desktop/src-tauri/tauri.conf.json`). An RC does not publish `latest.json`.

## Nightlies

Every day at 03:17 UTC the workflow builds `rc`'s tip and publishes
image tags `nightly` (moving) and `nightly-YYYYMMDD` (frozen, kept). No
GitHub Release — 365 prereleases a year is tag noise with no reader; the
dated image tags carry the history. Dated tags are cheap: each is one
manifest over shared blobs.

Re-run one by hand from Actions → release → Run workflow (nightly from
`rc`; `rc` pushes the moving `rc` tag from the rc tip — a dispatch cannot
invent a version, only a tag carries one).

If nightlies ever just stop, there are two things to check, and the second is
the one that bit: GitHub disables schedules after 60 days of repo inactivity —
but a run can also fail to START. A `startup_failure` produces **no jobs and no
logs**, so it looks like nothing happened at all; that is how this channel sat
dead from 2026-09-05 to 2026-09-16, with the run list the only evidence. Open
the run's own page and read the banner (the API's log endpoints show nothing).
The cause that time was a workflow-validation rule: a called workflow's jobs may
request no more than the CALLING JOB grants, so the nested `api-package` call
was refused for asking `packages: write` under a `contents: read` caller. Those
grants now sit on the calls in `release.yml` — if a new nested call is added and
the tag or nightly dies as a startup failure, that is the first thing to look at.

The retired `testing` branch used to be this section's problem: it had to be
fast-forwarded by hand, nothing enforced that, and a stale channel published
stale code with today's date on it. `rc` cannot go stale the same way — it is
where every pull request lands, and `main` is kept current by the promotion —
which is the other half of why the nightly feed moved onto it.

Delete it when convenient: `git push origin :testing`. Until then, pushes to it
are refused by `scripts/flow-guard.mjs`, so it cannot quietly come back to life
as a second feed.

## The tags on `ghcr.io/outcrop-labs/talaria`

| Tag | Moves when | Immutable? |
|---|---|---|
| `main` | every app-touching promotion to main | no — a pointer |
| `sha-<sha12>` | every app-touching promotion to main | yes |
| `nightly` | every nightly build | no — a pointer |
| `nightly-YYYYMMDD` | every nightly build | yes |
| `rc` | every RC tag | no — a pointer |
| `X.Y.Z-rc.N` | its tag | yes |
| `latest` | every stable tag | no — a pointer |
| `X.Y.Z` | its tag | yes |

Pin anything you care about to the right-hand column.

The api package — `ghcr.io/outcrop-labs/talaria-api` — carries these same
tags, plus an immutable `sha-<sha12>` per commit; that sha tag is what a
release's app-image build is actually pinned to.

**What the api bits are compiled with.** `main` (the feed the in-app updater
rolls from) and every stable tag carry the api built for release. The two
pre-stable channels — `nightly` and `rc` — carry it built with cargo's dev
profile (`api/Cargo.toml`'s `[profile.dev]`: our crates `-O1`, dependencies
`-O3`, `debug = line-tables-only`). That is deliberate and it is a trade, not a
downgrade in disguise: those channels exist to smoke-test the same sources, and
the unoptimised build of our crates is ~2.5 minutes instead of ~25, which is
the difference between a nightly landing the same day and not landing at all.
It also means an RC is **not** a performance preview of the stable release —
`X.Y.Z` is built again from the tag, in release. Do not benchmark an rc image;
do not read a number off one.

Two more consequences of the dev profile, because they show up as bugs if you
don't expect them: debug assertions and overflow checks are ON (cargo's dev
default), so a nightly is also an assertion smoke-test — a panic you can only
reproduce on `nightly` may be an assertion that release code would have wrapped
through, not a defect in the release. And it is bigger and slower to start: the
binary is ≈417 MB against release's ≈146 MB (the package image 536 MB against
209), and the api takes a few seconds to bind rather than one, which the
compose healthcheck's 90-second start period absorbs. That is the price of the
compile being ~8 minutes instead of ~16.

## The desktop installers

Every tag publish also attaches the desktop app's installers to the GitHub
Release (`desktop-package.yml`, called by `release.yml` once the image push is
done — the assets land on a release that exists): linux
`.deb`/`.rpm`/`AppImage`/`.pkg.tar.zst`/`.flatpak`, macOS a universal `.dmg`
(and the `.app` zipped), windows an NSIS `.exe` and an `.msi`, plus a
`SHA256SUMS` over all of them.

Nightlies open no Release (above), so they carry no installers. A trunk build
that touches `desktop/` IS a desktop release — the auto-minor channel: once
every platform job is green, `desktop-package.yml` mints `desktop-vX.Y.0`
(highest suffix-free X.Y.Z across the `v*` and `desktop-v*` tags, minor+1,
patch 0) and opens a regular Release for it carrying the installers and
`latest.json` — the LIVE in-app updater feed, because `/releases/latest`
resolves to it and installed desktop apps update themselves. The `desktop-v*`
namespace keeps a desktop auto-minor from ever firing `release.yml` (its tag
trigger is `v[0-9]*`). Auto never crosses a major: **majors (and any
hand-picked number) are manual** — dispatch `desktop-package` with
`version=1.0.0` and `publish=true`. A stable `vX.Y.Z` cut raises the baseline
the same way; before cutting one, check it exceeds the highest `desktop-v*`
tag (the auto math resumes from whichever is higher). To re-attach installers
to an existing release, re-run that release's `desktop-package` job, or
dispatch with `version` and `release_tag` filled in — also the way to finish a
mint whose run was cancelled mid-attach; `--clobber` makes either idempotent.
A plain dispatch without `publish` builds artifacts only, nothing minted.

Nothing is signed or notarized yet. That is a provisioning decision, not an
oversight: it takes an Apple Developer certificate and a Windows signing key.
Until then macOS needs a right-click → Open the first time (Gatekeeper) and
Windows shows a SmartScreen warning.

## The trunk feed

Beside the channels, main publishes continuously: `.github/workflows/app-image.yml`
builds the app image for every push to main that touches running bits and
pushes `sha-<sha12>` (immutable) first, then `main` (pointer). The api stage
is pinned before the build — an api-touching push waits for that same
commit's `talaria-api:sha-<sha12>` (api-package's push run builds it in
parallel; the wait is the guarantee a trunk image never mixes a new UI with
an unbuilt api), anything else pins the digest `talaria-api:main` names.
Channel tags stay release.yml's: those are versions a human cut, not trunk
tips. `workflow_dispatch` exists for re-runs, not bootstrap — the merge that
lands the workflow touches a path it filters on, so the first run fires
itself, and it pushes to the package release.yml already made public.

## One-time setup notes

- **The api package is bootstrapped (2026-09-01, the cutover merge).** The
  merge push itself ran the first build (it touched `api/**`), so the `main`
  + sha tags existed from day one, and the package was born **public** — the
  org's package default — verified by anonymous pulls of both tags. A fresh
  environment replaying this setup: dispatch the workflow once by hand if
  the first push doesn't touch `api/**`, and check visibility like the app
  image below.
- **GHCR visibility.** The first push creates the package **private**.
  Flip it: github.com/outcrop-labs/talaria → Packages → talaria → Package
  settings → Change visibility → Public. Until then, pulls need
  `docker login ghcr.io` with a PAT that has `read:packages`.
- **Branches must contain the workflow.** A tag-push event resolves the
  workflow file at the *tagged commit*, so `rc` and `main` must include
  `release.yml` before the first tag is cut on them. Both were created from a
  main that already had it; a brand-new checkout of the process should mind the
  ordering. The same rule runs the other way for the branch model's own
  automation — `workflow_run` and `schedule` triggers only fire for workflow
  files present on the default branch, which is why
  [`docs/BRANCHES.md`](./docs/BRANCHES.md) → Bringing the model up puts the
  first promotion before the branch protection.

## Deliberately not

- No npm/PyPI publishing — the repo's published artifacts are the two
  images (the app and the api package) and the desktop installers, nothing
  else.
- No signed attestations, and no signed or notarized desktop installers (see
  "The desktop installers" above).
- No nightly-tag pruning (see above: negligible growth).
- No auto-changelog; CHANGELOG.md is hand-maintained, and PRs update it.
