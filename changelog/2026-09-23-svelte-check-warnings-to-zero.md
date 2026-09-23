- **svelte-check: 143 warnings → 0, 0 errors held.** The a11y debt from the
  2026-08-25 alpha-readiness audit is cleared across the touched surfaces:
  form labels are associated with their controls with real `for`/`id` pairs
  (the Admin panels — GitHub, Google client, org targets, org, email,
  encryption — plus the MCP, fleet and assistant modals; the secrets panels
  moved from raw `<input class=…>` to the `Input` primitive per the
  2026-08-17 UI-streamlining notes), clickable rows and menu surfaces got
  keyboard semantics, and BoardList was rebuilt without its invalid list
  nesting. Every remaining `state_referenced_locally` read was triaged: the
  one-time seeds carry a written justification comment, and the ones that
  were real bugs are fixed — the inbox panel kept serving a stale thread
  when the picker moved (its hook now follows the live selection, as does
  AgentsTab's eager `canEdit`). The contacts-app rows the audit named were
  dropped from the repo with the apps/ examples.

  Verified: `bun run verify` green — svelte-check 0 errors, 0 warnings over
  5398 files; 1237 UI tests; mcp tsc; invariants and changelog checks. A
  static audit of the diff confirms every new label `for=` matches an `id=`
  in the same file and no duplicate ids were introduced. Falsify check:
  reverting the annotation pass alone brings 36 warnings back; applying it
  returns 0.
