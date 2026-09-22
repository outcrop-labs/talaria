- **Release channels: nightly, RC, and stable images on GHCR.** A `rc`
  branch stages release candidates, a `testing` branch feeds a nightly
  (03:17 UTC), and one release workflow publishes both plus the stable
  path: a `vX.Y.Z-rc.N` tag on `rc` builds image `X.Y.Z-rc.N` + moving
  `rc` and opens a GitHub prerelease; a `vX.Y.Z` tag builds the version +
  moving `latest`; the nightly publishes `nightly` + `nightly-YYYYMMDD`
  with no Release (365 prereleases a year is tag noise with no reader).
  The channel resolution lives in one `resolve` job that fails loud on
  malformed tags — nothing silently falls through to `latest`. Publishes
  are gated by calling ci.yml as a reusable workflow, so "what gates a
  release" and "what gates a PR" are the same list and cannot drift. Git
  tags are the version authority (the `package.json` versions stay
  decorative and unread); the workflow never commits a version bump. The
  image carries its identity as OCI labels + `TALARIA_VERSION`
  (`unknown` on local builds, which is simply true of a local build).
  Operators consume a channel with a committed override —
  `docker compose -f docker/compose.yml -f docker/compose.registry.yml
  up -d --no-build` with `TALARIA_CHANNEL` pinning the tag — and the
  default checkout-build path is byte-identical to before (the base
  compose file changed comments only, verified by diffing
  `docker compose config`). The model, the tag grammar, and the
  one-time GHCR visibility flip: `RELEASING.md`.
