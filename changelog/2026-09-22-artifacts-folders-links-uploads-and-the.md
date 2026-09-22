- **Artifacts, folders, links, uploads, and the Drive export speak Rust.**
  Eleven routes over three proxy prefixes (`/api/artifacts` does NOT
  prefix-match `/api/artifact-folders` — character 13, `s` vs `-`), with the
  write half of the engine: version snapshots, the official→KB mirror
  through the real qdrant/embed deps, the first-publish slug mint, the
  streamed multipart capped BEFORE it buffers (a declared over-cap
  content-length answers 413 unread), and the Drive export proven live
  against production Google from both runtimes. The diff's finds:
  `artifact_links.target_id` is TEXT (three `::uuid` casts refused to match),
  and the PUT's title tri-state — absent means don't-touch, never set-null.
