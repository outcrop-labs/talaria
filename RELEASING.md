# Releasing Talaria

How the channels work, how a release is cut, and what the automation does.
The operator-facing half (running a prebuilt image) is
[`docs/CONTAINER.md` → Prebuilt images](./docs/CONTAINER.md); the machinery
lives in [`.github/workflows/release.yml`](./.github/workflows/release.yml).

## The model

**Git tags are the version authority.** Every `package.json` in this repo
says `"0.1.0"` decoratively and nothing reads them; the running version of
any install is the commit it runs (the Updates panel shows it), and the
version of any published image is the tag that built it. No release writes
its version into a file — there is no version-bump commit, and nothing to
merge afterwards. The one file a release changes is `CHANGELOG.md`: the
roll (`scripts/changelog-roll.mjs`) folds the `changelog/` entries into a
section for the version, and that commit reaches rc the way everything on rc
does — through a pull request — before the tag (see "Cutting an RC").

Three long-lived branches, one chain — everything on `main` is on `rc`, and
everything on `rc` is on `testing`. A version is cut only from the two that
are deployed and verified first:
[`docs/BRANCHES.md`](./docs/BRANCHES.md) is the model in full.

| Branch | Role | How it moves | What a tag on it means |
|---|---|---|---|
| `testing` | the integration lane: every feature pull request targets it, and `testing-deploy.yml` deploys every push to the testing instance | merging pull requests (the author, at green) and the back-flow from `rc` | nothing — a tag on testing is refused; its images are `testing` and `testing-<sha12>` |
| `rc` | the release candidate, and the staging environment: `rc-deploy.yml` deploys its tip | a frozen snapshot of `testing` (`promote/testing-<sha12>`), or a `hotfix/`, `release/` or `sync/` branch cut from `rc` — merge commits a person makes | `vX.Y.Z-rc.N` — a release candidate of what `rc` currently is |
| `main` | the release trunk — what customers run | only the promotion merge of `rc` into it, after `rc` is deployed and verified | `vX.Y.Z` — a stable release of what `main` currently is |

**The lanes into rc.** Features land on `testing` and are exercised there;
`rc` takes them as a batch, by snapshot — `gh workflow run promote-testing.yml
--ref main -f open=true` cuts `promote/testing-<sha12>` at testing's tip and
opens its pull request into `rc` (full CI on every platform, an approving
review). The snapshot never moves, so the battery runs once per candidate and
the commit merged is the commit checked. Besides it, `rc` takes only what is
cut from `rc` itself: a `hotfix/` (no migrations), the changelog roll
(`release/`), a sync of main (`sync/`). Whatever `rc` gains that way flows
back to `testing` automatically ("Promoting to stable", below).

What remains a human's call is the one thing that should be: whether a
version exists. A person decides it by merging the changelog roll's pull
request, and the tag follows the promotion's merge (see "Promoting to
stable").

## Cutting an RC

Two steps, in this order: the changelog roll, as a pull request, then the tag.

```bash
# 1. roll the changelog on a branch from rc, and open the pull request
git fetch origin && git switch -c release/roll-0.2.0-rc.1 origin/rc
bun scripts/changelog-roll.mjs 0.2.0-rc.1     # folds changelog/*.md into a section
git add CHANGELOG.md changelog/
git commit -m "changelog: roll 0.2.0-rc.1 — the entries since the last release"
git push -u origin release/roll-0.2.0-rc.1
gh pr create --base rc --title "changelog: roll 0.2.0-rc.1"
#    merge it as a merge commit once CI is green; rc-deploy deploys the merge
# 2. tag that merge
git switch rc && git pull                     # the tip you mean to cut
git tag v0.2.0-rc.1 && git push origin v0.2.0-rc.1
```

The roll comes first because the tagged tree must carry the version's
section: a checkout of the tag then says what it holds, and the promotion
carries the same section to main. The release's notes are built from that
section, and their full-changelog link points at the tag
(`blob/vX.Y.Z-rc.N/CHANGELOG.md`), so it resolves from the moment the tag
exists. It is a pull
request because rc moves only by merges — `scripts/flow-guard.mjs` refuses a
hand-made commit pushed to rc, in the pre-push hook and in `flow.yml` alike.
Merge no snapshot between the roll and the tag: it would ship in the tag with
its entries still under `changelog/`, outside the section. rc moves only on
snapshots, hotfixes, rolls and syncs, so that window is quiet by itself.

Nothing else needs advancing first: `rc` already holds the snapshot of
`testing` this candidate is, and its tip has been deployed and smoke-tested by
`rc-deploy.yml` on the way in — which is the guarantee an RC is supposed to
carry. Tag the tip whose deploy you
watched (`git log --oneline -1` against the run list under Actions →
rc-deploy).

The tag push runs the release workflow: the full CI suite against the tag,
then the image builds and lands on GHCR as `0.2.0-rc.1` and (moving) `rc`,
and a GitHub **prerelease** opens with notes from `scripts/release-notes.mjs`,
built from the `## 0.2.0-rc.1` section (a tag with no section gets a stub). No
desktop installers ride it: Talaria Desktop ships only as `desktop-v*`
releases ("The desktop installers", below). A re-run of the same tag rewrites
the notes, so change the entries, not the Release page.

A malformed tag (`v1.0`, `v1.0.0-beta.1`, `v1.0.0-RC.1` — the grammar is
`vX.Y.Z-rc.N`, lowercase, exactly) fails the workflow loudly rather than
publishing anything. So does a tag on the wrong branch. `release.yml` publishes
a `vX.Y.Z-rc.N` only when its commit is on `rc`, and a stable `vX.Y.Z` only
when its commit is on `main`. A tag cut on `testing` or on a feature branch is
refused before CI starts, because a release publishes only promoted code.
Delete a misfire by deleting the tag.

One trap when you re-cut a tag that already opened a release: **GitHub converts
that release to a draft** when its tag goes away, and the next run finds the
draft, refreshes its notes and uploads its assets into it — so the release looks
done to anyone with push access and is invisible to everyone else. Publish it
again once the re-run is green:

```bash
gh release edit v0.2.0-rc.1 --draft=false
```

## Surface releases: desktop and mobile

Two surfaces version themselves alongside the platform rather than inside it,
in their own tag namespaces, because each produces an artifact a person
installs and each needs a version an installer can compare:

| Surface | Namespace | Auto | Manual door |
|---|---|---|---|
| Desktop | `desktop-v*` | a `desktop/` push to main (or one to `ui/src/lib/dither-engine.ts`, which the launcher bundles) mints `desktop-vX.Y.0` (minor+1 across **both** `v*` and `desktop-v*`) and attaches the installers | dispatch `desktop-package.yml` with `version` + `publish=true` |
| Mobile | `mobile-v*` | a `mobile/` push to main mints `mobile-vX.Y.0` (minor+1 within `mobile-v*` only; first is `0.1.0`) and attaches the Android APK | dispatch `mobile-package.yml` with `version` + `publish=true` |

Neither writes its version into a file: both inject the resolved version into
the build the same way a platform release does, so the rule above holds — **no
release writes its version into a file.** Neither rolls the changelog either:
that belongs to the platform release.

The one place they differ from each other is what they count. Desktop tracks
the platform's version line, so its auto-minor reads the app's `v*` tags too.
Mobile is its own artifact on its own cadence and reads only `mobile-v*` —
which is also why its first mint is `0.1.0` rather than wherever the platform
happens to be.

**Every mobile release is a prerelease.** The APK is debug-signed with the
keystore that ships inside every Android SDK: sideloadable, not distributable.
`/releases/latest` must never resolve to one. A Play-signed build is a second
job in that workflow once store credentials exist.

## Promoting to stable

A stable release is two merges, after the snapshot of testing it releases
has landed on rc. CI does the rest: the version, the roll, the promotion, the
tag, the images and the notes.

0. **The snapshot.** A `promote/testing-<sha12>` pull request into rc (above,
   "The lanes into rc"), merged by a person; rc's CI and `rc-deploy` verify it.
1. **The roll.** When rc's tip is verified (CI and `rc-deploy` green) and
   `changelog/` holds entries, `promote.yml` opens a pull request against rc
   from `release/roll`. It folds the entries into a `## X.Y.Z (date)` section
   with `scripts/changelog-roll.mjs`. **Merging it decides the release.**
2. **The promotion.** rc verifies again with `changelog/` empty, and
   `promote.yml` opens `promote: rc → main — vX.Y.Z`. Merge it as a merge
   commit.
3. **The tag, the images and the notes.** On that merge, `tag-release.yml`
   sees main's CHANGELOG.md top section with no tag, tags main `vX.Y.Z`, and
   dispatches `release.yml` at the tag. A tag pushed with the workflow token
   starts no workflow, and a dispatch is the one event it can start.
   `release.yml` runs CI against the tag, publishes `X.Y.Z` and `latest`, and
   opens the GitHub Release with notes from `scripts/release-notes.mjs`.
4. **The back-flow.** The roll moved rc, so rc now has commits testing lacks.
   On rc's next CI completion `promote-testing.yml` cuts `backflow/rc-<sha12>`
   at rc's tip and opens `sync: rc → testing`; merge it like any pull request
   into testing. Until it merges, the next snapshot is refused (rc would hold
   the roll and testing would not) — and a promoted entry edited on testing
   is a judge `must`, because the roll already owns it.

**The version** comes from `scripts/release-version.mjs`:

- By default it's the next patch after the newest `vX.Y.Z` tag.
- An entry file raises it with a `<!-- bump: minor -->` (or `major`) line. The
  largest ask wins, and the roll strips the line.
- A different version: Actions → promote → Run workflow from main, with
  `version` set. It must be newer than the newest tag.

**An entry that lands after the roll** stays in the same release:

- The roll's section is "pending", newer than every tag. The next verified rc
  gets a new roll pull request that extends that section, re-headed with
  today's date.
- An open promotion is held until it merges. `flow.yml`'s promotion gate fails
  while `changelog/` has entries, auto-merge goes off, and the body says what
  it's waiting for. `tag-release.yml` also refuses to tag a main that carries
  unrolled entries.

**The notes** are deterministic. Reviewers saw everything they say in the
pull requests:

- The images, the newest `desktop-v` release, and the pull requests behind
  the section's entries.
- **Before you upgrade:**
  - the migration count, from `pg.ts`'s `MIGRATIONS` at the previous tag and
    this one;
  - every entry's `Upgrade:` paragraph. An entry that needs an operator step,
    such as rolling agents or setting a variable, says it there.
- **What changed:** each entry's bold lead, then the full section, Verified
  paragraphs included.

**The first release after the chain landed** is rolled by hand, the way
0.13.2 was. Until that promotion, main's `promote.yml` is the old one and
never opens a roll, while rc's new promotion gate already refuses a head with
`changelog/` entries. So roll by hand (below) and promote; `tag-release.yml`
arrives on main with that same merge and tags it.

**What needs `PROMOTION_TOKEN`:** the roll and promotion pull requests. With
no token, `promote.yml` prints the exact commands instead, and you open them
yourself: a pull request the workflow token opens would start no checks. The
tag and the release need no token.

**By hand**, when CI can't (no token, or a release to cut out of turn):

```bash
# 1. the roll, as a release/ pull request against rc (merge commit), like everything on rc
git switch -c release/roll-0.2.0 origin/rc
bun scripts/changelog-roll.mjs 0.2.0      # --extend to add to a pending section
# 2. once rc verifies, open the promotion and merge it as a merge commit:
#    tag-release.yml tags it. Tag by hand only if that run failed before
#    tagging (if it tagged but the dispatch failed, re-run it: it dispatches a
#    tag at HEAD that has no Release, or `gh workflow run release.yml --ref v0.2.0`):
git switch main && git pull
git tag -a v0.2.0 -m v0.2.0 && git push origin v0.2.0
```

A stable made after an RC needs no roll when `changelog/` is empty: the RC
sections already describe everything it carries. But then no section is
pending for `tag-release.yml`, so tag it by hand as above. A roll can never go
to main directly: main takes rc's content and nothing else (flow-guard).

Publishes `0.2.0` and (moving) `latest`, and a regular GitHub Release.
`latest`, the image tag, moves on nothing else — only a `vX.Y.Z` tag with no
suffix.

**The GitHub Release is never marked "Latest".** GitHub's Latest release is
the desktop updater feed: the shell reads
`/releases/latest/download/latest.json`, and the release behind that is the
newest `desktop-v*` one (the next section). GitHub would make a new stable
release Latest by default, and a `v*` release carries no `latest.json`, so
every installed app's update check would 404.
So `release.yml` creates and edits every `v*` release with `--latest=false`,
and `scripts/check-invariants.mjs` (`release-latest-is-the-desktop-feed`)
fails a workflow that drops the flag. Leave it that way in the Releases page
too.

A stable tag attaches no desktop installers and no `latest.json`: installed
apps keep updating from the newest `desktop-v*` release, and Desktop ships
nowhere else (below).

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

### `testing` (the integration lane, since 2026-11)

`testing` once fed this section: it had to be fast-forwarded by hand, nothing
enforced that, and a stale channel published stale code with today's date on
it — which is why the nightly feed moved onto `rc` and `testing` sat idle.
Since November 2026 it is the integration lane instead: every feature pull
request lands there, so it cannot go stale for want of someone remembering it,
and `rc` takes it by snapshot ([`docs/BRANCHES.md`](./docs/BRANCHES.md)).

It is still not a release channel. The nightly keeps building `rc` — the
registry's default channel is `nightly`, and integration code never reaches a
default install. `testing` has its own feed instead: every push runs
`testing-deploy.yml`, which publishes `ghcr.io/outcrop-labs/talaria:testing`
(moving) and `testing-<sha12>` (immutable) for the testing instance to follow
([`docs/TESTING-INSTANCE.md`](./docs/TESTING-INSTANCE.md)). No `v*` tag is
ever cut on it — `release.yml` refuses one. `scripts/flow-guard.mjs` refuses
its deletion or rewrite and judges every push to it.

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
| `testing` | every push to `testing` (`testing-deploy.yml`) — the testing instance follows it | no — a pointer |
| `testing-<sha12>` | every push to `testing` | yes |

Pin anything you care about to the right-hand column.

The api package — `ghcr.io/outcrop-labs/talaria-api` — carries these same
tags, plus an immutable `sha-<sha12>` per commit (`talaria-api:sha-<sha12>-dev`
when the build used the dev profile: nightly, rc, and the testing lane's
deploy — never the `testing` tags, which the api package does not carry); that
sha tag is what a release's app-image build is actually pinned to.

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

Talaria Desktop ships **only** as `desktop-vX.Y.Z` releases, minted by
`desktop-package.yml`. A server tag — `vX.Y.Z` or `vX.Y.Z-rc.N` — builds and
attaches no installers, and neither does a nightly. Desktop has its own version
line, and the in-app updater offers only a higher number: a Desktop stamped
0.13.0 from a server tag beside `desktop-v0.12.0` would never be offered an
update until Desktop passed 0.13. A `desktop-v*` release carries linux
`.deb`/`.rpm`/`AppImage`/`.pkg.tar.zst`/`.flatpak`, macOS a universal `.dmg`
(and the `.app` zipped), windows an NSIS `.exe` and an `.msi`, a `SHA256SUMS`
over all of them, and the `latest.json` and `.sig` files the updater reads,
which need `TAURI_SIGNING_PRIVATE_KEY` (the matching pubkey is in
`desktop/src-tauri/tauri.conf.json`). That key is a secret of the
`desktop-release` environment, which admits only `main`: not `desktop-v*` tags,
because anyone with write access can point a new tag at unreviewed code. So only
`main` signs or publishes. Once the repository-level copy of the key is deleted,
a `desktop-package` dispatch from any other ref builds every installer unsigned
and attaches nothing. That is how to exercise an edit to the workflow, which no
longer mints a release when it reaches main (only `desktop/` and the dither file
do).

A trunk build that touches `desktop/` IS a desktop release — the auto-minor channel: once
every platform job is green, `desktop-package.yml` mints `desktop-vX.Y.0`
(highest suffix-free X.Y.Z across the `v*` and `desktop-v*` tags, minor+1,
patch 0) and opens a regular Release for it carrying the installers and
`latest.json` — the LIVE in-app updater feed. The release is created with
`--latest=false` and marked GitHub's **Latest** explicitly once its
`latest.json` is uploaded, so `/releases/latest` (the only URL the updater
reads) moves to it only when there is a feed to serve, and never to anything
but a desktop release. Installed apps read it when someone checks for updates;
they do not update unasked. The `desktop-v*`
namespace keeps a desktop auto-minor from ever firing `release.yml` (its tag
trigger is `v[0-9]*`). Auto never crosses a major: **majors (and any
hand-picked number) are manual** — dispatch `desktop-package` from `main` with
`version=1.0.0` and `publish=true`. A stable `vX.Y.Z` cut raises the baseline
the same way; before cutting one, check it exceeds the highest `desktop-v*`
tag (the auto math resumes from whichever is higher). To re-attach installers
to an existing `desktop-v*` release, re-run its `desktop-package` run, or
dispatch it from `main` with `version` and `release_tag` filled in — also the way to finish a
mint whose run was cancelled mid-attach; `--clobber` makes either idempotent.
A plain dispatch without `publish` builds artifacts only, nothing minted.

A published `desktop-v*` release also opens
a pull request on [omapak](https://github.com/outcrop-labs/omapak) that moves
`app.talaria.desktop` to that tag and regenerates the vendored cargo and bun
sources (`omapak-submit.yml`). The store judge still scores it; the workflow
does not merge. That needs `OMAPAK_SUBMIT_TOKEN`, a secret of the same
`desktop-release` environment (a fine-grained PAT or GitHub App token with
contents and pull-requests write on `outcrop-labs/omapak`). Without it the job fails on purpose: a rolled
release that never reaches the store is a red run, not a quiet success.
Before any work the job asks GitHub's git server whether the token may push
(receive-pack's ref advertisement), and a token that may not fails there with
the reason: rejected outright, or valid without Contents write. Then
regenerate it and re-run the job by a dispatch from `main` with the tag.
A server `v*` release does not submit: it carries no desktop build. A
`desktop-v*` release published by hand runs at its tag, which the environment
does not admit, so its run fails with that same dispatch to make.

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
  files present on the default branch, which is why the testing lane's
  machinery (promote-testing.yml, testing-deploy's dispatch) reached main in a
  promotion before the cutover made `testing` the lane
  ([`docs/BRANCHES.md`](./docs/BRANCHES.md) → The cutover).

## Deliberately not

- No npm/PyPI publishing — the repo's published artifacts are the two
  images (the app and the api package) and the desktop installers, nothing
  else.
- No signed attestations, and no signed or notarized desktop installers (see
  "The desktop installers" above).
- No nightly-tag pruning (see above: negligible growth).
- No generated changelog, and no written-at-release notes. Every pull request
  writes its own entry under `changelog/`. The roll folds them into
  CHANGELOG.md through a pull request before a tag, and the release notes are
  assembled from that section (see "Promoting to stable").
