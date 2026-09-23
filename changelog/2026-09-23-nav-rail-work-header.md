- **The nav rail no longer prints a WORK header above the work views.** The
  sidebar section title repeated what every item under it already says —
  Inbox, Comms, Plan, Boards, Research, Knowledge, Files — and it stood in
  the way of a future view claiming that name (the Work-view epic, TALA-65,
  collided with exactly this). Decision from product: remove the section
  title and let the views speak for themselves. Manage keeps its header: it
  is the control plane, and its label still carries meaning.

  Sections now carry a stable `id` (`work`, `manage`) that consumers branch
  on; `title` is display-only and optional, and a section without one renders
  as a bare list. NavRail's Apps-placement and app-manage slotting branch on
  `id` instead of the display string, so nothing anywhere matches on "Work"
  as a string. The top strip's breadcrumb drops the "Work /" segment for the
  same reason: a caption repeating an unspoken label reads as a bug, not
  restraint. `Manage / Agents` and `System / Settings` breadcrumbs stay.

  Deliberate deviation from the Mercury spec (§5), which names the sidebar
  sections WORK/MANAGE/SYSTEM: the Work header is gone by product decision;
  the spec text should be read accordingly.

  **Verified:** `bun run check` green; `bun run typecheck` (svelte-check +
  tsc) green, 0 errors, no new warnings in the touched files; `bun run test`
  1186 passed across 72 files. Real-browser proof on the stubbed shell (24
  assertions, all passing): Work header absent, MANAGE header present, all
  seven work views + manage items rendered, unread badges live (Comms 2 /
  Plan 1, fetch proven at the wire), breadcrumbs `Inbox` / `Comms` /
  `Manage/Agents`, collapsed rail shows zero headers and re-expands with
  Manage intact. Screenshots: TALA-70.
