- **Skills install from the Hermes marketplace, per agent, in the Studio.** The Studio's
  "Knows" section grows a marketplace button (beside Teach) that opens a picker modeled on
  the MCP marketplace: Hermes Atlas's ranked catalog (`hermesatlas.com/lists/top-skills`)
  searched live, destination picked per agent or the shared root, and one-click install —
  a single-skill repo installs on the click, a pack opens a preselected checklist so one
  more click installs the lot. Installing downloads the repo's tarball from codeload,
  scans it for SKILL.md directories (the agentskills.io unit, support files riding along,
  nested packs staying separate), normalizes names into the skill alphabet, and writes them
  into the owner's mounted root — Hermes reads skills per invocation, so installs are live
  with no container restart. Never clobbers: a name that already exists there reports
  "already present" and is left exactly as it was. `GET /api/skills/marketplace`
  (the list, `?q=` filtered), `GET …/detail?repo=` (one repo's discovered skills), `POST
  …/install` (the write, behind the same canEdit gate as the skill PUT); the engine is
  `api/crates/talaria-skills-marketplace` with hostile-path refusal, per-file/per-skill/
  whole-repo size caps, and a bounded cache so the discover→install pair downloads a repo
  once. The listing's summary fallback also learned agentskills.io frontmatter — a
  `description:` in a leading `---` fence wins and `name:` keys no longer leak as the
  one-liner.

  Verified: `bun run verify` green (check with regenerated `docs/api`, svelte-check, 1186
  ui tests) and `bun run api:check` green (fmt, clippy `-D warnings`, workspace tests —
  14 new unit tests across the marketplace crate, the fallback, and the installer's
  path rules); exercised in the running dev stack with a minted session — the catalog
  lists all 49 entries and search narrows it; `tlehman/litprog-skill` one-click-installed
  into an agent (full tree on disk under `fleet/agents/<slug>/skills/`, library row with
  the frontmatter description), reinstall answered `exists`, a 6-of-7 selective install
  from `obra/superpowers` and `conorbronsdon/avoid-ai-writing` packs through the picker
  both landed and refreshed the Knows list in the browser; the test skills were deleted
  afterwards, leaving the agent's root as found.
