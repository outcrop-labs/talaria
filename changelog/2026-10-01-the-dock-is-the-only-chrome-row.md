- **The black bar under the dock is gone, and Manage opens.** The dock era
  shipped a second full-width band beneath the primary nav — darker than the
  dock itself — carrying search, the notification bell and the account chip,
  plus at most one word of breadcrumb. Two stacked chrome bands for one row of
  controls was the thing the dock was meant to end. The three controls moved
  into the dock's right cluster, behind a divider that reads as the seam
  between the workspace and the person, and `TopStrip` was deleted: 101px of
  chrome across every view becomes 56px, and the view now starts at the dock's
  bottom edge. The two views with no tile anywhere — Settings and Admin, which
  live in the account menu and render no title of their own — light that chip
  instead, which is the job the deleted "System" breadcrumb was doing.

  **The Manage sidebar could not open at all.** Three bugs, each hidden behind
  the one before it:

  1. `ManageSidebar` read its open state as `const { manageOpen } =
     useManageSidebar()`. That hook returns a getter over module `$state`, so
     destructuring evaluated it once at component init and froze the answer at
     `false` forever. The gear lit (`TopDock` holds the object and stayed
     live) and nothing appeared — from the gear, from the account menu's
     Manage entry, from anywhere.
  2. The close-behind-navigation effect asked only "is the current path a
     manage view?", which is true the whole time you are standing on one — so
     opening the pane from `/models` closed it in the same flush. It now fires
     on a CHANGE of route, and tests "did you land on a row this pane has"
     (`activePath`) rather than scanning `MANAGE_VIEWS`, which missed every app
     manage surface the pane also carries.
  3. That effect ignored `pinned` entirely, so the pin only ever changed its
     own icon; and nothing cleared the pin on close, so a reopen came back
     pinned and the pin button's first click unpinned. Both fixed.

  Also: the pane derived its rows with `isAdmin: false` hardcoded while the
  dock used the real role, under a comment promising the two "can never
  disagree" — inert today (no manage item is admin-only) and a trap the first
  time one is. It takes `user` now, like the dock.

  Verified in a running stack (worktree `nav-merge`, :5302, real Redis session,
  admin user), driven in headless Chromium before and after. **Before:** 2
  full-width chrome bands totalling 101px, and the Manage pane never entered
  the DOM — not from the gear, not from the account menu, not while standing on
  `/models`. **After:** 1 band of 56px with content starting at y=56, and 18/18
  browser checks pass with no console errors — search, bell, account chip and
  gear all inside `nav[aria-label="Primary"]`; the gear opens the pane from
  Home AND from `/models`; the account menu's Manage entry opens it; all 8
  manage rows render; Escape closes; a row navigates and the unpinned pane
  closes behind it; a pinned pane survives the navigation; closing clears the
  pin; the chip lights on `/settings`; the dock's search still expands into a
  focused input and still stays out of `/boards`. Plus `bun run check`,
  `bun run typecheck` (svelte-check, 5425 files, 0 errors) and the ui unit
  suite (86 files / 1372 tests).

  Known debt left standing, pre-dating this change: `lib/view-title.svelte.ts`
  has had no reader since the dock dropped the title row, and a dozen comments
  around `ViewHeader`, `Rail`, `RailSurface` and `StageHeader` still say "the
  top strip names the view". The comments naming the deleted file are
  corrected; rewriting that family's prose is its own pass.
