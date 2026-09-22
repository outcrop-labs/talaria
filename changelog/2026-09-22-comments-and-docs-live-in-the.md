- **Comments and docs live in the present.** The port-era narration is gone
  from the tree: ~470 Rust/TS files' comments no longer cite their TS
  originals, batch/wave provenance, cross-references to deleted files, or a
  parity battery that finished — each now states its own contract directly
  (wire shapes, key order, invariants, ordering and parse-order facts,
  security reasoning, test pins all kept). Stale port TODOs whose conditions
  long since landed were deleted rather than rewritten (registry's
  still-to-cross ids, define's DEFERRED block, artifacts' batch-5 notes,
  render's ensure_mcp_service). What remains TS-named is deliberately so:
  the four permanent residents' rule-10 facts, the live TS twin contracts
  (`ui/src/server/tasks.ts`'s agent-authority predicate, `task-const.ts`,
  the secretbox cross-language cipher), and `mcp/src/index.ts` sync pins.
  Every changed line verified comment-only by a diff proof over the whole
  tree. The docset follows: HARNESSES.md now points at the Rust harness
  layer with the TS twin framed as the app-author plane; API-CONVENTIONS.md
  makes the Rust crate the runtime of record with the TS dialect governing
  the residents and app servers, and names both homes of the HITL
  predicate; ARCHITECTURE, AGENT-NETWORKING and DEVELOPERS read current;
  CONTAINER.md's fleet-manifest link points at `api/src/fleet/render.rs`
  (was the dead TS renderer); and the port's own narrative moved to
  `docs/history/rust-port.md` when RUST-MIGRATION.md split into living
  rules + frozen record. Old references live exactly where they belong
  now: this changelog and `docs/history/`.
