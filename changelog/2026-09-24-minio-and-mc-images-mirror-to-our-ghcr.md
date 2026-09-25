- **MinIO is dead upstream — minio and mc images are now digest-pinned to our
  own GHCR mirror.** github.com/minio/minio is archived (2026-04-25) and mc
  with it (2026-07-14); the last releases (server RELEASE.2025-10-15,
  mc RELEASE.2025-08-13) were never published to any container registry, and
  quay's tags are frozen at the 2025-09 builds — post-freeze hotfix tags point
  at commits that do not exist in the public repo. A `:latest` from a dead
  project can move but never be fixed, and the old "watch quay's freshness in
  #366" posture was watching a frozen thing. Every reference now points at
  `ghcr.io/outcrop-labs/talaria-minio@sha256:14cea…` (the frozen quay latest,
  RELEASE.2025-09-07) and `ghcr.io/outcrop-labs/talaria-mc@sha256:a7fe…`
  (RELEASE.2025-08-13): docker/sidecars.compose.yml (all three stacks layer
  it), `talaria box seed`, and the backup/restore mc fallback — one spelling
  of each pin, in `cli/src/backup/lib.ts`. A new `minio-mirror` workflow
  skopeo-copies both digests from quay into GHCR (`--all`, multi-arch
  preserved, tags `latest` + the matching RELEASE) and asserts the pushed
  digest equals the source. `TALARIA_MC_IMAGE` still overrides, except a
  value pointing at the deleted `docker.io/minio` namespace — that warns and
  falls through to the pin. Verified: `bun run check` green; `cd cli && bun
  run test` and `cd cli && bun run typecheck` green, including the new
  mcImage branch tests (override honored / dead-namespace warned + pinned)
  and the box-seed create-argv assertion; all three compose pairings parse
  via `docker compose … config -q` (the devbox file with its `BOX_*`
  interpolation variables set, as the box CLI always sets them). The mirror
  workflow itself has NOT been dispatched yet — that happens from the PR
  branch after push (and the packages must then be flipped public by hand),
  so the GHCR digests are not yet registry-verified; every pull of the
  pinned refs depends on that run.