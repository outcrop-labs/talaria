- **The built-in bucket now runs versitygw, not MinIO — and backups use rclone,
  not mc.** MinIO withdrew public distribution outright: the repository is
  archived (2026-04-25), the `docker.io/minio` namespace was deleted,
  `quay.io/minio/minio` and `quay.io/minio/mc` now answer **401** to an
  anonymous pull, and `dl.min.io` returns **410 Gone**. The 2026-09-24 mirror
  was the right instinct and never ran: `minio-mirror.yml` failed on its first
  execution and every one since, on a missing `docker://` transport prefix on
  skopeo's *source* argument, so `ghcr.io/outcrop-labs/talaria-minio` was never
  published. A compose file pinned at a package that does not exist is what
  `deploy update` hit — `docker compose up -d` died with `error from registry:
  denied`, on an instance whose containers were otherwise healthy. Re-mirroring
  was not available as a fix: the sources are gone, and nothing public remains
  to copy. The sidecar is now
  `ghcr.io/versity/versitygw:v1.8.0` (Apache-2.0) on its posix backend, which
  maps S3 keys straight to files — `uploads/a.pdf` is
  `/data/<bucket>/uploads/a.pdf` — so the blobs stay readable with `cat` and
  backed up with `tar` if this project ever goes the same way. The service is
  renamed `minio` → `storage` on a **new** `storage-data` volume (the two
  engines share no on-disk format, and a posix backend pointed at `minio-data`
  would serve MinIO's `.minio.sys`/`xl.meta` internals as objects);
  `minio` stays a network alias so an operator who pinned
  `TALARIA_S3_URL=http://minio:9000` keeps working, and `TALARIA_MINIO_PORT`
  / `TALARIA_MINIO_CONTAINER` still seed the new names so no dev loop moves.
  Carrying an existing bucket across is a documented two-step rclone copy
  staged through the host — **docs/BACKUPS.md § "Migrating off MinIO"** — which
  must run while the old container can still be started. `mc` died with the
  server, so `talaria backup`/`restore`/`box seed` now shell to
  `rclone/rclone:1.71` (MIT), configured entirely through
  `RCLONE_CONFIG_<REMOTE>_*` env: no alias step, no config file, and the
  credentials never reach a command line. `copy`, never `sync` — `mc mirror`
  did not delete extras at the destination and `rclone sync` does. A leftover
  `TALARIA_MC_IMAGE` is refused with a warning rather than honoured, since the
  argv is rclone's. Also fixed alongside: `talaria deploy update` pulled the
  registry images **twice** (`runUpdate` pulled, then `runUp` pulled again —
  visible as the duplicated `docker compose pull` in the incident transcript),
  and the dead `minio-mirror.yml` is deleted. Verified: every four operations
  `api/crates/talaria-storage/src/lib.rs` signs — CreateBucket, PUT, GET,
  DELETE, path-style SigV4 — replayed against a live versitygw with a harness
  first validated 5/5 against the running MinIO, including the
  `BucketAlreadyOwnedByYou` repeat-create that `ensure_bucket` already
  tolerates by body match, so **no Rust change was needed**; the real
  `rcloneRun` from `cli/src/backup/lib.ts` round-tripped a nested key
  byte-identically in both directions through the Docker fallback path and
  returned false (not threw) on an unreachable endpoint; `bun run gate` green
  — `check`, `tsc --noEmit`, 216 cli tests; all three compose pairings render
  via `docker compose config` (the devbox file with its `BOX_*` variables set,
  as the box CLI always sets them). Not done here: the published app image and
  TEI are both amd64-only, so arm64 hosts remain unsupported — unchanged by
  this work, but it surfaced during the sidecar image audit — and it does not
  affect the dev loop, which runs the app on the host and containerises only
  the sidecars. versitygw itself was confirmed on arm64 by running the
  `linux/arm64` variant (`uname -m` = aarch64) through the same 5/5
  conformance suite, so Apple Silicon machines run the new sidecar natively;
  docs/BACKUPS.md now spells out the macOS host-client install and which
  single sidecar (TEI) still needs emulation.
