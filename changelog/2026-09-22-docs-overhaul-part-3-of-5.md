- **Docs overhaul, part 3 of 5 — the full SDK docset.** `docs/sdk/` replaces the single-file
  `docs/SDK.md` (now a redirect stub, so existing links survive): getting-started, ui-kit,
  client, server, mcp, harnesses (including the previously undocumented bridge pattern:
  `runHarness`/`resolveHarnessModel` from app code), workbench-harnesses (the two-contracts
  table), and `reference.md` — every export of both entry points, one row each. New tripwire in
  `check-docs.mjs`: SDK export coverage diffs both directions against `reference.md`, so a new
  export without a row fails `bun run check`, and so does a documented symbol that doesn't
  exist (this caught `KeyHint`, which the old doc listed for months). Surface repairs the
  coverage pass surfaced: `Popover` now actually exported from `@talaria/sdk`;
  `CheckResult`, `ToolPolicy`, `ModelChainStep`, `ResolvedHarnessModel`, `RunLedger` exported
  from `@talaria/sdk/server` (all referenced by already-exported types). The harness example
  and the bridge example in `docs/sdk/harnesses.md` are transcribed verbatim into
  `ui/src/sdk/server.test.ts` and run through the real runner — the documentation is a build
  target. `docs/APPS.md` rewritten to the Svelte 5 anatomy, linking the docset.
