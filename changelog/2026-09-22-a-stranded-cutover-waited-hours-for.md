- **A stranded cutover waited hours for a reader that could never come.**
  `reconcile_boot` is the only writer that finishes a cutover, but its
  callers were the 6h check (whose interval lease a dead container's ghost
  holds for the whole interval) and the admin read — which arrives through
  the very edge a stranded cutover has not raised; green publishes no port
  of its own. And when reconcile *did* run while the old container still
  held the port, the edge-raise lost the bind race and the whole reconcile
  errored instead of recognizing the hold. A new `update-reconcile` job —
  one reconcile a minute, one settings-row read when idle — now retries
  the handover however it stranded, and the hold with the port still held
  answers as the hold it is: the edge rises at the cut, and the next tick
  lands the run.
