- **The integrations/google family speaks Rust — the whole OAuth pair, the
  org plane, the user surfaces, the approval queue, and the agent plane,
  twenty-one routes under one prefix.** Every door reads the same
  connection rows, so the family crossed whole: the personal
  connect+callback ladder (five deterministic refusal rungs), the org
  plane (its own connect pair, status/targets/disconnect, provisioning,
  live health probes), the per-surface user reads with their query-param
  folds, the three user writes that draft through the approval queue
  rather than send, the pending decide, and the agent surfaces —
  `refuse_legacy` first, the `x-agent-name` must-equal-model check, then
  a resolution rule where a personal assistant acts strictly as its
  owner's connection and a fleet agent as the shared org account. The
  gmail engine crossed as its own module; verified by a 69-case byte-diff
  against TS on the same sessions plus two agent keys, including live
  reads against the real org connection (calendar, drive, gmail, labels,
  one full message) and the one designed round-trip: an agent drafts a
  send, a member's verdict is refused, the admin's reject retires it —
  nothing leaves the building. The diff caught two port bugs: the
  targets wire's key order (embed the typed struct — its declaration
  order is the wire order) and the provisioning filter that seats only
  org agents in the send-as table. Also the port's first silent-trait
  lesson: a non-`Send` form serializer held across an await in an engine
  fn fails every handler that awaits it with a bodyless E0277 — scoped
  and finished before the await now.
