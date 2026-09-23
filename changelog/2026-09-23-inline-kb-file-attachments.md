- **Files live inside the document, not beside it.** A KB doc body now takes
  inline attachments: paste, drop, or pick a file at the cursor ("/" → Attach
  file) and it renders in place as part of the body — images as the image
  itself, every other file as a labeled chip with its name and size that opens
  the shared file viewer (preview + download, never a browser tab). The
  reference is plain markdown in the body (`![name|size](upload:<id>)`), so
  saves, edits, and re-ordering can't orphan or duplicate the file, and
  read access stays exactly the doc's — the upload serves only to viewers who
  can already read a document that references it.

  Verified: new unit pins (placeholder renders, pipe-in-filename intact,
  round-trips the editor spelling, ordinary links/images untouched, re-order
  keeps one chip per reference); svelte-check clean; full ui suite green
  (1192 tests); `bun run check` exit 0.