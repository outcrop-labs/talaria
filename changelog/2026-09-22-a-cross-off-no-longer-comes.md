- **A cross-off no longer comes back with tomorrow's date on it.** The brief
  is one document per day, and a check-off lived only inside the day's own
  append-only log — so every morning's open re-listed everything still live
  in its sources, including the rows the owner had crossed off the day
  before, un-crossed, forever. The owner's verdict now carries: the open and
  the sweep skip a key whose newest entry on a prior day is the owner's own
  check or dismissal with an unchanged source fingerprint, for thirty days.
  The item reappears the moment its source actually moves (the task fails
  differently, the thread gets a new message, the draft is rewritten) — the
  same rule the within-day sweep always closed lines by — and `restore`
  still works from the prior day's page, which the read serves before the
  day's own brief opens. Verified against the live database
  (`api/tests/brief_verdict_carry_live.rs`, ignored like every live proof):
  a checked-off blocked task is absent from the next day's document, a
  stale-fingerprint verdict does not suppress, and a restore lands on the
  prior page and is re-added by the next sweep.
