- **Every page 404'd in production while `/api` kept working.** The server
  build now splits into `dist/server/assets/*.js` chunks — one directory
  deeper than the `dist/server/server.js` the SPA-shell lookup was anchored
  to — so `readFile('../client/index.html', import.meta.url)` threw, the
  handler cached `shell = null`, and all four deployed instances (dogfood ×3
  + bbills) served plain `404 Not Found` for `/`, `/home`, `/login`, … Dev
  mode never runs the shell path (vite serves `index.html` itself) and no
  gate executed the built bundle, so CI was green on it. The shell now
  resolves from `process.cwd()` (`ui/` in dev and under server-entry alike —
  the rule `app-build/paths.ts` states). Verified: `bun scripts/check-prod-shell.ts`
  red on the pre-fix build, green after; `bun run check`; ui test + typecheck.

### Added
