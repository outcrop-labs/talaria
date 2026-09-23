- **Fresh deploys no longer die pulling minio — MinIO removed its Docker Hub
  namespace, and every minio image reference now points at quay.io, which
  hosts the same builds.** Symptom, from a customer deploy: `pull access
  denied for minio/minio, repository does not exist or may require 'docker
  login'` — verified upstream from two independent networks: the Docker Hub
  repo 404s (the whole `minio` namespace, `minio/mc` too) while
  `quay.io/minio/minio:latest` is digest-identical to the last Hub-published
  build and pulls fine. Swapped `docker.io/minio/minio` → `quay.io/minio/minio`
  in the deploy, dev, and devbox compose files, and `minio/mc` →
  `quay.io/minio/mc` in `talaria box seed` and the backup sidecar's image
  default — that last one would have broken existing installs' backup jobs on
  their next mc pull, not just fresh deploys. Verified: `bun run check` green
  (gen-docs current), tests 1118/1118, typecheck clean outside the gitignored
  client subrepos (known local-only leak; CI is truth), all three compose
  files parse via `docker compose config -q`, and both quay images pull on
  this box (mc build 2025-09-07; `TALARIA_MC_IMAGE` still overrides).
