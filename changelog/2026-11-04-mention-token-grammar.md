- **@-mentions resolve for every email shape again.** A mention stopped
  resolving for anyone whose email localpart carried a dot, an underscore or
  a plus sign: the composer inserted the raw localpart while the server's
  token scanner reads a grammar of letters, digits and dashes, so `@j.siegel`
  scanned to `@j` and nobody was ever notified (Jon's report, TALA-93). Both
  sides now speak the same normalized grammar — lowercase, every char
  outside `[a-z0-9]` mapped to a dash, runs collapsed, edges trimmed — and
  the agreement is pinned by tests on the notify crate (token shape plus an
  end-to-end picker-to-scan assertion), the mentions crate and the UI.
  Verified: `cargo test -p talaria-notify` 63/63 and `-p talaria-mentions`
  and `-p talaria-tasks` green from `api/`; `bunx vitest run
  src/components/chat/mentions.test.ts` 5/5 from `ui/` — the two
  implementation tests red before the fix, green after; the stale
  `ada.l` pin in talaria-mentions re-pinned to the normalized token it now
  correctly produces.
