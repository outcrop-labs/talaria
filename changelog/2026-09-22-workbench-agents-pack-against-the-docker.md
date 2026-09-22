- **Workbench agents pack against the docker host, not a 3-job stall.**
  Admit uses `docker info` MemTotal when the API is cgrouped smaller than
  the VM (`/proc/meminfo` when they match). Keep-back is 25% of that total
  (2–16 GiB). Ceiling is total − keep-back (never above 32 GiB);
  `oom_score_adj: 500`. Reservation sums each live job's effort (1/2/4 GiB).
  A refused start writes `work_wait` and the ticket API returns `{ session,
  wait }` — board cards and the ticker show queued position + reason; wait
  rows clear only when a session is actually live. Verified: `merge_host_mem`
  + `jobs_bytes` + `work_wait::wire` tests; `bun run check`.
