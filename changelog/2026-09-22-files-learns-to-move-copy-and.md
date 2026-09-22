- **Files learns to move, copy, and duplicate — the whole management
  grammar.** Cut/copy/paste for files AND folders: ⌘X/⌘C/⌘V, the selection
  bar's Cut/Copy/Move, row and breadcrumb context menus (Paste into), and a
  Paste entry on right-clicking empty space. Cut dims the spoken-for rows;
  Escape clears the clipboard before the selection (the more transient thing
  first); a cut pasted back where it came from is a silent no-op while a copy
  pasted in place duplicates — Drive semantics. Copy's engine is new on the
  server: POST /api/artifacts/{id}/duplicate and
  /api/artifact-folders/{id}/duplicate copy one record or a whole folder tree
  in one transaction (recursive CTE, cycle-guarded). Copies are the caller's
  and private, land beside their source under a "Copy of" name with a
  bounded collision ladder, share the source's storage blob (safe — no
  delete touches the blob), and never inherit public slugs, KB mirrors, or
  Google linkage. And the Move dialog is finally here: a navigable folder
  browser with breadcrumbs, a New-folder control, and self/descendant
  destinations dimmed and refused.

### Changed
