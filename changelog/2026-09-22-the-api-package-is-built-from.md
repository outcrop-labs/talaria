- **The api package is built from the api again — and the stub gate that says
  so.** `cargo chef` landed in `api/package.Dockerfile` on 09-19, and the
  `build` stage inherited `deps`' `WORKDIR /repo/api` — so its relative
  `COPY api ./api` landed at `/repo/api/api`, while the skeleton `cargo chef
  cook` had written (every manifest at `0.0.1`, `src/main.rs` = `fn main() {}`)
  stayed the only source cargo could see. Cargo relinked the skeleton and the
  package published a **544 KB binary that exits 0 and prints nothing**. Every
  app image built on it — main's `:main`/`sha-<sha12>` feed, the one the in-app
  updater rolls to — died at boot: `server-entry.ts` spawns the api, watches it
  exit, and exits with it ("RUST API EXITED (code 0)"), so the container
  crash-loops and the instance serves nothing. That is the 09-19 → 09-21
  window, and it is why an instance stopped serving when it was updated.

  The `COPY` names its destination absolutely now (`COPY api /repo/api`), and
  the build stage ends with the stub gate: it boots the binary it just built
  against an unreachable database and requires an HTTP answer on
  `/api/healthz` — 503 from a dependency that is down, 200 from a healthy one,
  because which status is the environment's business and *answering at all* is
  the api's. A skeleton cannot answer, so this class of breakage is a red build
  instead of a silent publish.

  **Not done**: the 09-18 → 09-21 nightly failures are a second, unrelated
  fault — `release.yml` calls today's `ci.yml` against the `testing` branch, and
  the ui job's prod smoke runs `ui/scripts/check-prod-shell.ts`, which `testing`
  (moved by hand, by design) does not contain yet. No nightly has published
  since 09-17. Moving `testing` forward is the documented fix, and that is a
  human's call.

  Verified: the package build now reports `Compiling talaria-api v0.1.0` (the
  real crate) where the broken build reported the skeleton's `v0.0.1`, and the
  artifact is 143.8 MB that names its missing config instead of exiting
  silently; the gate body was exercised against both binaries (real →
  `answers /api/healthz (HTTP/1.1 503)`; the 544 KB stub → `never answered on
  :5274`, exit 1); and an app image built with
  `--build-arg TALARIA_API_IMAGE=talaria-api:fixed` boots and serves — `/` and
  `/home/inbox` 200 `text/html`, `/api/healthz` 200 with `rustApi.ok: true` —
  where the published `ghcr.io/outcrop-labs/talaria:main` image exits 1 on the
  same command.
