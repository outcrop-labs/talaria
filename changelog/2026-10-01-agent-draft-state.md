- **TALA-11 (part 3): draft agents on the roster.** A hire made with "start
  later" writes the def, renders the fleet config, and skips the boot by
  design — but the roster showed it as 'down' (red dot), a lie about a
  deliberate state. Enabled+managed+created defs with no container at all now
  read as 'draft': a neutral ink dot with a tooltip saying what it is, a
  quiet DRAFT outline chip on the tile and the list row, and a START control
  labeled "finish the hire" (the same 'up' action, not a renamed one — Manage
  keeps the identity/soul editable over time, Retire/Delete stay for changing
  one's mind, and Stop/Restart/Roll are unreachable because nothing is
  running). Imported or unmanaged defs without containers stay 'down' where
  they belong; a live container always beats the hire's intent. No strip
  change: the hires endpoint holds a done hire for ten minutes and HiringStrip
  renders only live/failed rows, so a finished start=false hire disappears
  quietly and the draft chip IS the landing state.

  Verified: new health-mapping tests (7 cases: draft reading, container
  beats intent, imported/unmanaged/retired guards, neutral color, all five
  original states unchanged) pass; full `bun run test` green; svelte-check
  clean.
