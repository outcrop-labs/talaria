- **Consistency pins that ride along.** The Rust crates stop re-spelling
  ladders the constant crate already owns: `talaria-fitness-talaria-tools`
  (which had its own `PRIORITIES`/`EFFORTS`/`COLORS`) and
  `talaria-routes-comms`'s `channels_id_plan` now import
  `talaria-task-const`'s, which is what the api validates against. The
  cross-language pin in `ui/src/lib/task-const.test.ts` grows from one
  assertion to four: the priority ladder, the effort ladder, `talaria-statuses`'
  off-board list (as before) and mcp's `AGENT_STATUSES` — each read out of the
  other language's source, so a drift on either side fails the client's suite.

  `scripts/check-invariants.mjs` gains the app-DB container-name pin: the
  resident tier (`ui/src/server/app-db.ts`) and the CLI's backup
  (`cli/src/cmd/backup.ts`) both compose `talaria-appdb-<instance>-<slug>`, and
  a drift would surface as "that container is not there" on the day somebody
  needs the dump. It is a pin, not an import — one end reads `process.env` and
  the other a passed-in record — so it checks the shape: the prefix, both
  variables, the `'talaria'` fallback.

  **Not done**: the ports pin (it needs `cli/src/ports.ts`, which is W12's), and
  `talaria-fitness-world`'s bare `INBOX`/`ASSIGNED`/… consts — there is no
  home to import them from: the board statuses are DB columns with a virtual
  default list, not constants, so the crate's copy is its own wire vocabulary.

  Verified: `bun run check` (13 rules, 0 clusters), `bun run api:check`,
  `bun run verify`.
