- **The sidebar carries live unread counts for Comms, Plan, and Research.**
  One endpoint (`GET /api/unreads`) sums what's waiting in each part of the
  app — rooms plus agent chats under Comms, shared-plan turns under Plan,
  unseen research runs under Research — using the same predicates the pills
  on each surface show, so a badge can never disagree with the pills it
  summarizes. The counts move live over the shared event stream and
  self-heal on a 30-second floor; a failed read renders `!`, never a silent
  0. Opening a research run is the seen gesture: its notifications clear
  with it, and the badge and the bell empty together.
