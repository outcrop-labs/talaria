- **Hermes bundled skills stay classified, and every pack we prune occupies
  the name agents reach for.** Hermes ships Notion, Obsidian, Airtable, gh,
  gws, Himalaya, Box, xlsx, llm-wiki, raw coding-harness CLIs, and more —
  and adds packs on image updates. A six-path prune array in docker.rs
  silently let new conflicts in, and only `github` had a Talaria signpost,
  so a search for "notion" found a hole and the model improvised. Source of
  truth is `scripts/hermes-skill-authority.json`: every snapshot path is
  replaced, keepExact, or keepPrefix; unclassified fails `bun run check`.
  Replaced packs are `rm -rf`'d on every container roll (`hermes_skills::
  prune_paths`); a short SKILL.md at `scripts/skills/<signpost>/` occupies
  the Hermes `name:` (email, obsidian, notion, airtable, google-workspace,
  box, xlsx, llm-wiki, claude-code, codex, opencode, xurl,
  teams-meeting-pipeline — github already existed). Fitness: `hermes:authority`
  (six fixtures — Notion/Obsidian/Excel/Box/wiki/Airtable asks must hit
  Talaria tools). keepPrefixes is apple/ only — every other family is
  keepExact so a new creative/ or web/ pack cannot sneak in. The chassis boot
  smoke `find`s SKILL.md in the live image and fails on unclassified packs.
  `update_document` takes `rows`/`html` and refuses markdown on a sheet or
  page (that would smash the grid). Soul-header bullets generate from
  `TALARIA_TOOLS` so a new tool cannot miss the contract. `hermes:authority`
  fails a reply that called the right tool then claimed "saved to Notion".
  Verified: `bun run check`; `cargo fmt`; `cargo clippy --lib -- -D warnings`;
  `cargo test --lib` hermes_skills, hermes_authority, talaria_tools, sandbox,
  org, registry.
