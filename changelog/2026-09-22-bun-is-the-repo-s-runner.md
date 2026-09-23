- **Bun is the repo's runner, dev through production.** A root `package.json`
  is now the hub for every way to drive the repo — `bun run dev` (full
  stack), `build`, `start` (production server on the Bun runtime), `test`,
  `typecheck`, `check` (invariants), `verify` (all of it) — and both packages
  carry `bun.lock` (migrated from their npm lockfiles; versions unchanged).
  setup/dev install and build through bun, the MCP server runs its TypeScript
  entry directly in dev (`bun src/index.ts`; tsx dropped) and production
  boots with `bun server-entry.js`. Node stays a hard floor — vite, vitest,
  svelte-check, and tsc run under it — so CI installs both, pinned to bun
  1.4.0. Verified live: `bun run verify` green from the root (2559 tests, 0
  type errors, invariants pass), a full MCP stdio handshake under bun, and a
  production boot serving real API traffic against live Postgres/Redis with a
  clean SIGTERM shutdown (the scheduler's 9 jobs drained).
