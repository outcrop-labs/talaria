- **The last plain-JS sources are TypeScript now: `server-entry`, the svelte
  config, and the service worker.** `server-entry.ts` is the one that
  matters — it was outside the tsconfig `include`, so the production server
  (env loading, static serving, the SSE pump, boot migrations, the Rust-api
  supervisor) had never been typechecked; it now carries full types (the
  dist bundle import is typed from `src/server/app.ts`, the one module
  whose exports survive into it). `svelte.config.ts` is supported natively
  by vite-plugin-svelte 6. The service worker moved from `public/sw.js`
  (served verbatim) to `src/sw.ts` as a second client-build entry emitted
  unhashed at `/sw.js` — the registration URL in browser-notify.ts is
  unchanged — with a small dev middleware serving the same URL so
  dev-mode registration keeps working. `bun server-entry.ts` replaces
  `bun server-entry.js` in the start script, the container entrypoint, and
  the image's runtime COPY.
  The stdlib-only `.mjs` under `scripts/` and `docker/` stay as they are:
  they must run under any node with no install, which `.ts` would break.
  Verified: `bun run verify` green (typecheck now covering the entry), a
  built-from-scratch `dist/` emits `sw.js` at the client root, and a boot
  of the built server against a scratch database listens, answers
  `/api/healthz`, and serves `/sw.js` with a JavaScript content type.

### Fixed
