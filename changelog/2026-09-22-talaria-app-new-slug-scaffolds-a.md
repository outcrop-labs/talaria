- **`talaria app new <slug>` scaffolds a TypeScript app** (work surface, document-store
  server, MCP starter) into `apps/<slug>`. Authors stay on `@talaria/sdk`; never Rust.
  The fleet skill `talaria-apps` is the playbook for building one. Verified: slug
  guard refuses before write; existing dest dies; files land under `apps/<slug>`
  (or `TALARIA_APPS_DIR`); skeleton embeds the slug in the client only.
