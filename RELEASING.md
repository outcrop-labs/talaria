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

Three long-lived branches:

| Branch | Role | How it moves |
|---|---|---|
| `main` | trunk, always shippable | PRs (rebase-merged, CI-gated) |
| `rc` | staging — what an RC is cut from | a human fast-forwards main into it |
| `testing` | the nightly feed | a human fast-forwards main into it |

To advance a channel:

```bash
git push origin origin/main:rc        # or :testing
```

No automation moves `rc` or `testing`, on purpose. They exist so a person
vouched for what's in them; an auto-promotion would defeat that.

## Cutting an RC

```bash
git push origin origin/main:rc                 # advance staging, if needed
git tag v0.2.0-rc.1 && git push origin v0.2.0-rc.1
```

The tag push runs the release workflow: the full CI suite against the tag,
then the image builds and lands on GHCR as `0.2.0-rc.1` and (moving) `rc`,
and a GitHub **prerelease** opens with a stub body. The changelog is the
record — edit the release notes afterwards if you want them to say more,
or leave the stub pointing at CHANGELOG.md.

A malformed tag (`v1.0`, `v1.0.0-beta.1`, `v1.0.0-RC.1` — the grammar is
`vX.Y.Z-rc.N`, lowercase, exactly) fails the workflow loudly rather than
publishing anything. Delete a misfire by deleting the tag.

## Promoting to stable

Same shape with a suffix-free tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

Publishes `0.2.0` and (moving) `latest`, and a regular GitHub Release.
`latest` moves on nothing else — only a `vX.Y.Z` tag with no suffix.

## Nightlies

Every day at 03:17 UTC the workflow builds `testing`'s tip and publishes
image tags `nightly` (moving) and `nightly-YYYYMMDD` (frozen, kept). No
GitHub Release — 365 prereleases a year is tag noise with no reader; the
dated image tags carry the history. Dated tags are cheap: each is one
manifest over shared blobs.

Re-run one by hand from Actions → release → Run workflow (nightly from
testing; `rc` pushes the moving `rc` tag from the rc tip — a dispatch
cannot invent a version, only a tag carries one).

If nightlies ever just stop: GitHub disables schedules after 60 days of
repo inactivity. That is the first thing to check.

## The tags on `ghcr.io/outcrop-labs/talaria`

| Tag | Moves when | Immutable? |
|---|---|---|
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
  workflow file at the *tagged commit*, so `rc` and `testing` must include
  `release.yml` before the first tag is cut on them. Both branches were
  created from a main that already had it; a brand-new checkout of the
  process should mind the ordering.

## Deliberately not

- No npm/PyPI publishing — the repo's published artifacts are the two
  images (the app and the api package), nothing else.
- No signed attestations.
- No nightly-tag pruning (see above: negligible growth).
- No auto-changelog; CHANGELOG.md is hand-maintained, and PRs update it.
