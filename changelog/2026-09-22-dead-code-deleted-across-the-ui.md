- **Dead code deleted across the UI, cli and their docs (~440 lines, no
  behaviour).** Every one was verified by grep before it went: no importer, no
  invariant anchor, no test.

  * `ui/src/server/permissions.ts` (192 lines) and its 214-line test — the TS
    permission catalog and `requirePerm` gate. The live catalog is
    `api/crates/talaria-permissions/src/lib.rs` now; the file survived only as a
    re-export in `api-guard.ts` nothing imported. `docs/PERMISSIONS.md` and
    `docs/API-CONVENTIONS.md` stop calling it "the catalog" and name the Rust
    twin of each guard.
  * `ui/src/lib/notify-classes.ts` — the routing/digest half
    (`isNotifyRoute`, `isNotifyClass`, `KIND_CLASS`, `notifyClassOf`,
    `resolveNotifyPrefs`, `DIGEST_PREF_KEY`, `storedDigestPref`,
    `digestEnabled`): zero importers; `talaria-notify` owns the vocabulary and
    the derived answer. What stays is what the client draws from —
    `NotifyClass`, `NotifyRoute`, `NOTIFY_CLASSES`, `NotifyPrefs`,
    `DigestPref`. (238 → 151 lines.)
  * `TERMINAL_KINDS` / `isTerminal` in `daily-brief-types.ts` and
    `RawFocusItem` in `inbox-focus-types.ts`. **Not** the two exported arrays
    the plan named: `BRIEF_SECTIONS` and `BRIEF_ENTRY_KINDS` are the *source* of
    `BriefSection` / `BriefEntryKind` (`typeof X[number]`), so they are
    load-bearing, not dead.
  * `cli/src/paths.ts`'s `isNewer` and its `boxDir(devboxes, name)` — the
    latter shadowed by `cli/src/cmd/box/shared.ts`'s `boxDir(ctx, name)`, which
    is what every caller imports.

  Verified: `bun run verify` (check + svelte-check 0 errors + 1,183 vitest tests
  pass), `bun talaria --help` still renders. No running-app pass: this host has
  no docker, so the dev stack (and therefore any visual check) is unavailable.
