- **The app shell moves to a top dock, and Manage gets a sidebar.** The left
  sidebar is gone: the primary nav is now a full-width band of icon tiles
  affixed to the top (the collapsed rail's grammar laid horizontally — same
  h-9 tiles, tooltips naming the view, badge counts). Manage leaves the row:
  a gear+"Manage" entry in the user menu opens a Manage sidebar (modeled on
  the old sidebar, hosting the eight manage views plus app-manage surfaces),
  toggled by a gear tile on the dock. The boards submenu left the shell and
  became a sidebar inside the boards view. Search moved to the strip as an
  icon beside the bell that expands into the input; page-title headers and
  the copy-link row are gone, and views take the full space (link/copy
  affordances return later, integrated per view). SidebarSearch, NavRail and
  the nav-rail state module are retired. Verified: svelte-check 0 errors, the
  ui unit suite (80 files / 1290 tests), prod build + SPA-shell smoke, and
  the repo `check` gate (invariants, docs, routes, changelog) — all green.
  No dev stack was driven on this box, so no in-browser pass; the dock,
  Manage sidebar, boards sidebar, and strip search are exercised by the
  reviewer in `bun talaria dev`.
