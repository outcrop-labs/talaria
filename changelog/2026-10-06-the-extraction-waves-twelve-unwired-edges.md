- **Twelve injected edges a crate-extraction wave declared and never wired, so
  four features had written nothing for eighteen days.** Between 2026-09-18 and
  2026-09-19 a burst of extractions moved a dozen modules into their own crates,
  each correctly breaking its new crate's dependency on a heavier one by
  declaring a `OnceLock` edge — and not one of them was set at boot. Every one
  of those commits verified with `cargo check`, which is exactly the tool that
  cannot see this: the declaration compiles, the read compiles, and the `None`
  arm is a valid answer. Usually the permissive one, so the feature reads as
  working.

  **The live instance dated the damage, and the counts were the trap.**
  `skill_summaries` has 29 rows, `gap_reported` notifications 15, `titler`
  harness runs 49 — all healthy-looking totals. The timestamps are the evidence:
  every one of them last wrote on **2026-09-18**, the day its crate was
  extracted, while `harness_runs` overall stayed active to 2026-10-05. So the
  Titler named nothing, no skill summary was written, and **nobody was told
  about a capability gap** — the one promise the honesty loop's queue makes.

  What each silence cost, now restored:

  | Edge | Unset, it meant |
  |---|---|
  | `AUDIENCE` | a gap filed and nobody told |
  | `COMPUTE_ALERT_COUNT` | an admin's Home always reading 0 alerts — indistinguishable from a healthy instance |
  | `GENERATE_TITLE` | every chat and plan keeping its mechanical first-message truncation |
  | `SUMMARIZE_SKILL` | nothing ever written to `skill_summaries` |
  | `MAYBE_DISPATCH_TICKET` | a ticket entering an agent-start column dispatching nothing |
  | `UPDATE_TASK` | every update through it refusing with "task update is not wired" |
  | `ROOM_COMMENT_FANOUT` | an agent's reply in a task room never becoming a ticket comment |
  | `SYNC_PRIVATE_DOCS` | a fresh personal assistant indexing none of its owner's private docs |
  | `WORKBENCH_TOOLS` | agents offered an empty Workbench tool list |
  | `RESOLVE_KB_REF` / `RESOLVE_ARTIFACT_REF` | every `@`-reference resolving to nothing — identical to "forbidden", and not the same thing |
  | `REF_BLOCKS` | an agent re-reading a thread losing the references it had already been given |

  Each is the pre-extraction call restored, recovered from the commit that
  removed it rather than reinvented. The two ref resolvers restore their
  **permission checks** along with the read, so a reference to a doc the reader
  may not see still resolves to nothing — the fallback was right about the
  outcome and wrong about the reason. Two signatures had drifted and are handled
  rather than papered over: `update_task` answers `Option<Task>`, so a miss is
  now a refusal instead of being flattened into a success, and
  `sync_user_private_docs` wants the retrieval edges, which this layer supplies
  as every other indexing caller does.

  **`UNSET_SEAM_CENSUS` is now empty**, which is the point — it was a backlog
  that could only shrink, and the `oncelock-seam-never-set` invariant's only job
  from here is to refuse the thirteenth.

  Verified: `cargo test -p talaria-jobs` 2 passed, with a boot assertion per
  edge naming what its silence costs — and they are real gates, not decoration:
  deleting the `GENERATE_TITLE` wiring makes
  `register_all_declares_the_whole_ported_table` panic. `cargo clippy -p
  talaria-jobs --all-targets` clean; `bun run gate` green, reporting **27
  `OnceLock` seams declared, 0 unset**. The dark window was established from the
  live instance's own tables, not inferred from the code.
