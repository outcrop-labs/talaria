- **`deploy update` pulls the api package before it builds.** The app image
  never compiles the api — it bakes in the pre-built package named by the
  Dockerfile's `TALARIA_API_IMAGE` — and `up -d --build` resolves that FROM
  from the local daemon cache. So an update on a checkout host could git
  pull, rebuild, recreate, and report success while still serving an api
  binary from before the pull: new UI wrapped around an old api (bbills,
  2026-09-03 — one day-old package answered for three "already fixed"
  symptoms at once). `talaria deploy update` now `docker pull`s the package
  the Dockerfile names, between the git pull and the build — the ref is
  read from the ARG line itself, so the pull and the build can't disagree —
  and a failed pull dies instead of baking the stale digest on purpose.
  `deploy up` keeps its old leniency: that path is also the
  reboot-a-broken-box path, and must not suddenly demand the registry.
