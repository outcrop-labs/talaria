- **Adoption — the one-time handover that turns the engine on.** An admin
  (or the migration script, through the machine key) presses Take over
  updates and the install stops being orchestrator-deployed: adoption pulls
  the image it should be (the registry's newest, or a re-pin of the running
  image's own digest when the registry is silent), renders the
  updater-owned project from the live container's own inspect document,
  brings slot A up behind the same health gate a roll uses, moves the fleet
  alias, and records the handover crash-safe before any port changes hands.
  Then the port decision: a fresh port (the infra path) puts green and the
  edge up on a port nobody holds while blue keeps serving — the second
  adopt call, which can only arrive THROUGH the edge, is the proof the
  proxy moved, and it stops blue; zero interruption at the origin. The
  inherit path (the panel's button) takes a one-time cut of a few seconds,
  named in the confirm dialog: the dying container arms a host-side helper
  (the app image itself, docker-cli baked in) that raises the edge the
  second the port frees, with green's reconcile as the crash fallback.
  adopt on an adopted install is the resume — every crash heals on the next
  call. The retired container is stopped by the finish, removed by the
  reconcile, and nothing resurrects it. Still true everywhere else: an
  install that never adopts keeps deploying exactly as it always has.
