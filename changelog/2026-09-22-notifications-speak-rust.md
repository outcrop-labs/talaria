- **Notifications speak Rust**: `/api/notifications` — the bell's one read
  (inbox, unread count, routing prefs, the digest answer, the instance email
  switch, and whether this user may flip it), mark-read by ids or all, the
  per-class routing patch, and the admin delivery switch. The mail side of
  the prefs (sendGatedMail, the bounded outbox, the drain) is scheduler
  plane and stays TS until batch 5; the port ends at the rows and switches
  the SPA reads and writes, plus the brief-nudge without its SSE publish.
  The prefs patch is the port's first zod record body — its full 400 table
  probed against the ui's own zod and pinned in Rust tests — and the route's
  ordering invariants held byte-for-byte against TS on the same DB: ids
  absent and ids empty both mean "mark all of mine", the member's delivery
  403 fires after the 400s and before any write (one PATCH never
  half-applies), and the admin flips write identical audit rows on both
  runtimes.
