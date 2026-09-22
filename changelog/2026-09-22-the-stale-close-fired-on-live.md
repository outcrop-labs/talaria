- **The stale-close fired on live adoption holds — and its wrong closes
  never healed.** A run in flight past an hour closed as failed even when
  *this container* was the run's own retired orchestrator, alive and
  serving the hold: the canary sat eighty minutes at a green adoption
  hold, both doors 200, and the row said failed. The exemption is the
  retirement itself — that run is ours and we are its proof of life. And
  for rows the close already marked failed while the containers finished
  the handover anyway, the reconcile now heals them to `done` when it can
  prove the landing (this container is the rolled-to digest, nothing
  retired is running, and the edge answers) — by hand no longer.
