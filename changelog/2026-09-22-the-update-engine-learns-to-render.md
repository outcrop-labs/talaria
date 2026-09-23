- **The update engine learns to render its slots.** The updater-owned
  compose project now has a shape: one derived from the LIVE container on
  every roll (env verbatim minus a denylist — the new image's version and
  install signal must win, and the updater kill switch is dropped because
  adoption is what flips it — same-path binds, docker gid, dns, the
  sidecar network joined as external so it stays owned by the project that
  runs the sidecars), two flip-rendered app slots with the compose
  healthcheck carried over byte-identical (the edge's docker provider
  refuses starting/unhealthy containers — that check IS the roll gate) and
  identical traefik labels so both slots merge into one health-gated
  service, and a pinned traefik edge that owns the host port the slots no
  longer publish. The docker verb set it drives ships beside it
  (inspect-self, per-service slot up, digest pulls, health waits, graceful
  stop-not-remove, alias attach). The image stops baking
  `TALARIA_UPDATER=off` — it contradicted the install signal (an adopted
  slot would have inherited it through the image's own env and the engine
  could never act); dormancy is the adoption gate, and operators who want
  the hard switch set the env themselves. Still nothing routes to any of
  this: no behavior change for any running install.
