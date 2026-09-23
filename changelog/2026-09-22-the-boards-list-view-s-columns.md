- **The boards list view's columns sit under their headers again.** Every
  row carried the hover dither field (`dither-fill`), and the field's
  canvas was inserted as a DIRECT child of the `<tr>` — but a table row
  admits only cells, so the browser wrapped the stray canvas in an
  anonymous cell of its own: a whole phantom column ahead of the checkbox,
  walking every real column one to the right of its header. The header said
  Status over nothing, the status pill sat under Priority, priorities under
  Assignees, and the timestamps hung past the last header with none at all.
  The canvas now hosts inside the row's first cell — it is absolutely
  positioned against the row either way, so the full-row field is unchanged
  (verified: 0 painted cells at rest, the full row painted on pointerenter).
  One fix, every table that carries the field on a row: board list rows,
  the fitness matrix's rows, and the public artifact tables.
