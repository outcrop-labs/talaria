---
name: cut-release
description: Cut a release — advance rc/testing, tag vX.Y.Z or vX.Y.Z-rc.N, and know what the release workflow publishes to GHCR. Use when cutting an RC or stable release, re-running a nightly, or diagnosing why a channel or image tag did not move.
---

# Cut a release

RELEASING.md is the contract — this is the working procedure distilled from it. The one
fact that shapes everything: **git tags are the version authority.** No package.json is
read by anything; releasing never edits a file — no version-bump commit, no changelog
commit, nothing to merge afterwards.

## The branches

| Branch | Role | How it moves |
|---|---|---|
| `main` | trunk, always shippable | PRs, CI-gated |
| `rc` | staging — what an RC is cut from | a human fast-forwards main into it |
| `testing` | the nightly feed | a human fast-forwards main into it |

No automation moves `rc` or `testing` — they exist so a person vouched for the contents.

```bash
git push origin origin/main:rc        # or :testing
```

## Cut an RC

```bash
git push origin origin/main:rc                 # advance staging if needed
git tag v0.2.0-rc.1 && git push origin v0.2.0-rc.1
```

The tag push runs the release workflow: full CI against the tag, images land on GHCR as
`0.2.0-rc.1` + (moving) `rc`, and a GitHub **prerelease** opens with a stub body pointing
at CHANGELOG.md — edit the notes after if you want more, or leave the stub.

**Tag grammar is exact:** `vX.Y.Z-rc.N`, lowercase, exactly. `v1.0`, `v1.0.0-beta.1`,
`v1.0.0-RC.1` all fail the workflow loudly rather than publishing. A misfire is deleted
by deleting the tag.

## Promote to stable

```bash
git tag v0.2.0 && git push origin v0.2.0
```

Publishes `0.2.0` + (moving) `latest` and a regular GitHub Release. `latest` moves on
nothing else — only a suffix-free `vX.Y.Z` tag.

## Nightlies

Every day 03:17 UTC, the workflow builds `testing`'s tip: tags `nightly` (moving) and
`nightly-YYYYMMDD` (frozen, kept). No GitHub Release — 365 prereleases is noise; the dated
image tags carry the history. Re-run one from Actions → release → Run workflow. **If
nightlies just stop:** GitHub disables schedules after 60 days of repo inactivity — that
is the first thing to check.

## The images

| Tag | Moves when | Immutable? |
|---|---|---|
| `nightly` / `rc` / `latest` | their build or tag | no — pointers |
| `nightly-YYYYMMDD` / `X.Y.Z-rc.N` / `X.Y.Z` | their build or tag | yes |

Everything is `ghcr.io/outcrop-labs/talaria` (the app) and `ghcr.io/outcrop-labs/talaria-api`
(the api package — same tags **plus** an immutable `sha-<sha12>` per commit; that sha tag
is what a release's app-image build is pinned to).

Production instances run from a separately operated infrastructure deployment — this repo
ends at the published images. The changelog is already maintained by PRs; releasing adds
no notes work.
