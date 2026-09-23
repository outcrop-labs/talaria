- **Workbench task concurrency is capped, with the sweep as the queue** — the
  fix for the 2026-09-17 outcrop freeze, where the engineering agent was
  offered 14 tickets at once, started 11 workbench jobs, and the accumulated
  per-job processes (vite, two Chrome clusters, tsservers, a Playwright
  install) OOM-killed its 4 GiB container 26 times until every work session
  blew its turn lease. Dispatch now offers a WORKBENCH agent at most 3 live
  work sessions (agents without a workbench are uncapped — their sessions are
  just model turns); a ticket past the cap is simply re-offered by the 60s
  sweep within a minute of a slot freeing — no queue table. `start_job`
  refuses a 4th live job naming the ones it has; approving a heavy plan at the
  cap 400s (reject always allowed). Workbench agents also render with
  `mem_limit: ${AGENT_WB_MEM_LIMIT:-8g}` (their sandbox runs builds, dev
  servers, and browsers; overridable per deployment like `AGENT_MEM_LIMIT`,
  lands on roll). Verified: cargo tests; dev stack — rendered compose carries
  the WB limit only on workbench agents; a workbench agent at 3 live sessions
  gets no 4th, and the ticket dispatches ~1 min after a slot frees;
  workbench-less agents dispatch past 3 unimpeded.

### Changed
