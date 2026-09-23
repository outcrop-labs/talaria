- **CI smokes the built bundle.** The `ui` CI job now runs
  `bun run build` + `bun scripts/check-prod-shell.ts`: it imports the real
  `dist/server/server.js` and asserts GET `/` (and a deep client route)
  serve the SPA shell and that `/api` paths never leak it. This is the gate
  that would have caught the 404 outage at PR time instead of on the fleet.
