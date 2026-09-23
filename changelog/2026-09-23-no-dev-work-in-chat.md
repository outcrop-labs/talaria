- **No dev work in chat — the rule is now load-bearing.** An agent recently
  started editing a codebase straight from a chat thread: no ticket, no
  workbench, no review surface. The policy is now stated everywhere agents
  and their configurers look: every rendered SOUL.md carries a standing
  dev-policy header (rule, routing, out-of-band changes still route through
  a ticket, chat instructions captured as ticket comments), the fleet-wide
  talaria-toolkit skill teaches the reflex, the workbench docs state the
  chat boundary, and the user guide's rails table spells out what agents
  can and cannot do. All dev work flows through a ticket and a workbench
  job — no exceptions for small changes.

  Verified: `cargo test -p talaria-org -p talaria-fleet-render` (22 pass,
  including the new dev-policy header test), `cargo clippy -- -D warnings`
  clean, `cargo fmt` clean, and `bun run check` green (invariants, doc
  links, generated references, changelog roll).