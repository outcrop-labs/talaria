- **KB documents can carry files inline, at the point in the prose where they belong** — a paperclip
in the doc editor, or just paste/drop a file: images (png, jpg, gif, webp) embed as visible
images exactly as before, and every other file becomes an inline chip showing its filename and
size that downloads on click. The chip is real markdown — `[filename](attachment://<id>?s=<size>&m=<mime>)`
— so it round-trips through save/load, survives re-editing and re-ordering, and stays readable
to agents reading the doc body via the API (filename as link text, mime and size in the href,
bytes at `/api/uploads/<id>`). Read mode renders the same token as the download chip through the
shared markdown pipeline, so editor and reader never disagree about what a doc contains.

Along the way this fixes a latent ACL bug the feature leans on: the uploads crate's
KB-doc read seam (`KB_DOC_ALLOWS_READ`) shipped unset after the uploads-crate extraction —
its resolver was never registered — so an upload embedded in a doc body answered **404 for
every non-owner reader** (the same disease `CONVERSATION_OWNER` had). Boot now wires the
seam (`register_all`, and the test boot seam), mirroring the pre-extraction logic:
doc → effective perms → `can_read`, with team grants; it fails closed.

**Verified:** `bun run check`, `bun run typecheck` (0 errors), `bun run test` (1205 passed,
+19 new across the token grammar and the read-renderer suites), `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test -p talaria-uploads`. The
`uploads_live` regression that pins the seam (doc reader gets 200, stranger gets 404) is
`#[ignore]`d per house rule — it needs the live stack. Exercised end-to-end in a real
browser against the running vite app with a stubbed api: a stored token renders as a
download chip with filename and human size in read mode, parses back into the inline editor
chip on open, and the byte-identical token survives a full edit → save → reload cycle
(evidence: 7/7 checks, screenshots in the agent workspace).