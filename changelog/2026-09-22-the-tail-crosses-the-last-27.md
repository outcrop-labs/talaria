- **The tail crosses — the last 27 TS route files proxy to Rust; coverage
  closes at 216 of 219.** The remaining singles and the fleet defs detail
  trio crossed as one sweep: the alert feed, home cards, global search (the
  hand-rolled SearXNG federation over a local FTS pass), the Muse, the
  inference ledger read, the join-code claim, the instance card at
  `well-known/talaria-instance`, `me/assistant` (the agent-start trio), the
  gap ledger (list + `{id}` resolve), the template library (list/create +
  `{id}` patch/delete), the skill registry (list + `{owner}/{name}` CRUD),
  the plans doc/members pair (with the plan-engine trio
  `ensure_plan_doc`/`plan_tier`/`sync_plan_doc`), the agent plane under one
  character prefix (`agent/gap`, `agent/problem`, `agent/message-user` — the
  refusal paths 403 on a fake taskId before any ticket/board write — plus
  `agent-media` both files and the role templates), the image describer
  `/api/vision/describe`, the remembered-facts plane `memory/{id}`, and the
  hire editor's versioned trio (`defs/{id}`, `/edit`, `/versions` —
  `revertTo` of the current version writes nothing and answers
  `created:false`; a fake endpoint 400s before any write). With the sweep
  the proxy's residents are exactly three, all permanent: `admin/update`
  (it rebuilds `ui/dist` and restarts the bun process it runs in —
  redesign at cutover), the rule-10 app dispatch, and `healthz` (served by
  both, never proxied). The byte-diff harness — 85 rows, 0 fail, 3
  documented world-dependent divergences (alerts/home read rows their own
  loops wrote; search federates live engines) — found four port bugs, all
  fixed: zod's enum message for template kinds (`Invalid option: expected
  one of "ticket"|"plan"` via `enum_member`), the skills file order (node's
  readdir SORTS where tokio's `read_dir` returns raw getdents order — both
  listing sites now sort), `me/assistant` PATCH's render propagation
  (`renderFleet()` has no catch in `updatePersonalAgent`, so a render
  failure 400s the whole PATCH with the version row already written and
  standing; only docker restart/up stays best-effort), and the bare
  secretbox message (`ensureAgentApiKey` propagates `open(existing.keyEnc)`
  unwrapped — no wrapper sentence).
