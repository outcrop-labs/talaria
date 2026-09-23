- **The inbox.focus family speaks Rust — the assistant's attention plane
  crosses whole.** The focus queue, badge summary, viewed/snooze state,
  card actions (with their confirmation tokens), the SSE command stream
  that runs an instruction through the focus assistant, and the segmented
  conversation picker serve from Rust under one proxy prefix — the
  family's per-user lock spans the command stream and the state route, so
  the prefix carries all eight paths. The engine crossed with them: the
  four-source queue builder (approvals, tasks, comms, notifications) with
  buckets and briefs, the action policy, the timeline, and the approvals
  read; the assistant itself is a harness def and became the streaming
  seam's first caller (`dispatch_transport` — gateway or fleet persona by
  kind — plus `StreamOptions`). One recorded divergence, documented at the
  route: on a client disconnect TS leaves the assistant row `streaming`
  while the port finishes and persists the reply (the chat family's tee
  philosophy). Verified by a 31-case byte-diff against TS on the same
  sessions — the reads across anon/member/admin, the validation shapes,
  and the writes through per-side seeded rows (mark_read consumes its row,
  so each runtime acts on its own notification), plus three SSE command
  streams byte-identical via a manifest-absent model's canned reply. The
  diff caught six port bugs, worst among them a member leak in the
  approvals source: TS splices the admin arm out of the WHERE clause for
  members entirely, and the port's first shape (`or is_org = $2`) matched
  every other person's personal actions. Also fixed: SSE frames carrying
  `conversationId` snake_case (an enum's `rename_all` camelCases only the
  variant tags — `rename_all_fields` does the fields; pinned by a test
  because the compiler cannot see the difference), two Postgres
  param-type 500s and a UUID-decode 500 in the picker, the summary's key
  order, and the command stream's header order.
