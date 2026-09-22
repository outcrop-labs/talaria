- **A work session waits out a working agent instead of hanging up on it, and
  a dead session no longer strands the ticket.** A session turn is an agent
  driving its tool loop — clone, build, test, iterate — and honest dev work
  runs for HOURS; the step's wall-clock ceiling (11 minutes, sized for a
  different world) killed the session mid-turn while the agent was
  demonstrably working (last model call 67 seconds before the kill), and the
  ticket sat in_progress silent for seven hours because dispatch only fires
  when a ticket ENTERS a pickup column. Three changes: the runs engine gains
  an IDLE-BASED step deadline (each ping on the step's activity tap re-arms
  the clock; only silence abandons the step — the law the chat transport
  already lived by, carried into the engine), the work session adopts it at
  ten minutes of silence while the persona stream pings per chunk; and a
  `work-redispatch` sweep re-offers every pickup-column ticket with agent
  assignees to dispatch every minute, whose generation walk stands down on
  live sessions and advances on dead ones — a dead session now costs a
  minute of silence, not an evening. The dispatch prompt also tells the agent
  to sync its checkout to latest `origin/main` BEFORE working (`git fetch`
  alone does not move the working tree — the incident's agent was about to
  base work on an eleven-day-old tree). Verified: engine tests pin both idle
  halves (a pinging step outlives the wall clock; a silent one is abandoned
  at the idle ceiling with a sentence that names it), and the session def
  carries `idle_step_ms` by assertion.
