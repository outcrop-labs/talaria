- **Files can manage Google Drive itself — rename, move, and trash, in place
  and on Google's side.** The personal connection's scope moves from
  drive.readonly to full drive (existing connections keep browsing read-only
  until one reconnect; the Drive place shows a "Reconnect Google to manage
  files" banner while that stands, and the write verbs stay hidden — a
  disabled verb reads as broken, a hidden one reads as not-yours-here). The
  org connection already carried full drive, so the Workspace Drive is
  manageable today; org writes require an admin, the same discipline the org
  connection's grant implies. Rename is the house prompt, Trash sits last
  behind the separator with a confirm (labeled Trash — Drive's trash is
  restorable), both on rows and on multi-selections. Drag-to-move works
  INSIDE the Drive place — folder to folder, onto the breadcrumb — routed
  through Google's addParents/removeParents, never Talaria's folders; a drag
  carries its source so a Drive drag can never hit the local move path. The
  Move dialog speaks Drive too: browses Drive folders, creates a Drive
  folder inline. Every write is audited (drive.rename | drive.move |
  drive.trash | drive.create_folder). Also gone: the no-op "Connect a
  source" rail row.

### Added
