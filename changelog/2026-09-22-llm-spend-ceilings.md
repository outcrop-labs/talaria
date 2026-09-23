- **LLM spend ceilings** (#243): rolling-window budgets in Admin → Settings — org-wide and
  per-caller, in tokens and/or priced dollars — checked before every gateway call (the HTTP
  route answers 429 `budget_exceeded` with `retry-after`; internal callers like the QA judge
  are held to the same ceiling). Off by default; a $ ceiling is never tripped by tokens with
  no price configured, and spend reads are cached briefly except at the edge (>80% of a cap
  goes exact, bounding what a burst can slip past). Cron schedules get a frequency floor
  (default: nothing faster than every 5 minutes) — a cron is an agent turn is spend.
