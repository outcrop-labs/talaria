import { existsSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig, loadEnv, type Plugin, type ViteDevServer } from 'vite'
import viteTsConfigPaths from 'vite-tsconfig-paths'
import { writeHeadHeaders } from './src/server/http'

const here = fileURLToPath(new URL('.', import.meta.url))

// Talaria UI — Vite + Svelte 5 SPA, sv-router on the client, and the API
// served from the same origin in both modes:
//   dev   the middleware below routes /api/* through src/server/app.ts
//   prod  server-entry.js wraps the built handler (see vite.server.config.ts)
// Tailwind v4 via the vite plugin; path alias `@/*` → `src/*` (see tsconfig).

/** /api/* (+ /.well-known/*) in dev, answered by the real server handler.
 *  ssrLoadModule keeps the server graph hot-reloadable like any other module. */
function apiDev(): Plugin {
  let server: ViteDevServer
  return {
    name: 'talaria-api-dev',
    config(_config, { mode }) {
      // `vite dev` historically loaded ui/.env into the server's process.env
      // (TanStack Start did this). Same rule as server-entry.js: the real
      // environment wins; the file only fills gaps.
      const fileEnv = loadEnv(mode, here, '')
      for (const [key, value] of Object.entries(fileEnv)) {
        if (!(key in process.env)) process.env[key] = value
      }
    },
    configureServer(s) {
      server = s
      server.middlewares.use((req, res, next) => {
        const pathname = (req.url ?? '/').split('?')[0]!
        if (!pathname.startsWith('/api/') && !pathname.startsWith('/.well-known/')) return next()
        void (async () => {
          const mod = (await server.ssrLoadModule('/src/server/app.ts')) as {
            default: { fetch: (request: Request) => Promise<Response> }
          }

          const url = new URL(req.url ?? '/', `http://${req.headers.host ?? 'localhost'}`)
          const headers = new Headers()
          for (const [k, v] of Object.entries(req.headers)) {
            if (v) headers.set(k, Array.isArray(v) ? v.join(', ') : v)
          }
          let body: Uint8Array<ArrayBuffer> | null = null
          if (req.method !== 'GET' && req.method !== 'HEAD') {
            body = await new Promise<Uint8Array<ArrayBuffer>>((resolveBody) => {
              const chunks: Buffer[] = []
              req.on('data', (c: Buffer) => chunks.push(c))
              // Copy into a plain Uint8Array<ArrayBuffer>: Buffer's ArrayBufferLike
              // backing store isn't assignable to fetch's BodyInit.
              req.on('end', () => resolveBody(new Uint8Array(Buffer.concat(chunks))))
            })
          }

          const response = await mod.default.fetch(
            new Request(url.toString(), { method: req.method, headers, body }),
          )
          // writeHeadHeaders (not the Object.fromEntries one-liner) so dev
          // matches server-entry.js on multi-cookie responses — Set-Cookie is
          // the one header that repeats, and the OAuth callback sends two.
          res.writeHead(response.status, writeHeadHeaders(response))
          if (!response.body) {
            res.end(await response.text())
            return
          }
          // Stream (SSE chat is the primary workload); stop pulling if the
          // client hangs up so upstream streams get cancelled too.
          const reader = response.body.getReader()
          for (;;) {
            const { done, value } = await reader.read()
            if (done) break
            if (res.destroyed || res.writableEnded) {
              await reader.cancel().catch(() => {})
              return
            }
            if (!res.write(value)) await new Promise((r) => res.once('drain', r))
          }
          res.end()
        })().catch((err) => {
          console.error(`[api-dev] ${req.method} ${req.url}`, err)
          if (!res.headersSent) {
            res.writeHead(500, { 'content-type': 'text/plain' })
            res.end('Internal Server Error')
          } else if (!res.writableEnded) {
            res.destroy()
          }
        })
      })
    },
  }
}

export default defineConfig({
  // Per-worktree Vite cache. Worktrees symlink the main node_modules (fast), but
  // that would SHARE node_modules/.vite — which concurrent dev servers corrupt.
  // A linked git worktree has a `.git` FILE at its root; those get a local cache.
  cacheDir:
    process.env.VITE_CACHE_DIR ??
    (existsSync('../.git') && statSync('../.git').isFile() ? '.vite' : undefined),
  // Dev server reachable over the LAN/Tailscale. allowedHosts only affects the
  // dev server (not prod builds); `true` lets IPs and hostnames through.
  // fs.allow ..: Talaria app codebases live in ../apps and compile into this
  // build (import.meta.glob) — the dev server must be allowed to serve them.
  server: { host: true, allowedHosts: true, fs: { allow: ['..'] } },
  build: { outDir: 'dist/client' },
  // App codebases have no node_modules of their own; shared deps resolve from
  // the host's — the peer-dependency model (one Svelte, one router, one query
  // client across the whole deployment). dedupe (not alias) keeps Vite's
  // normal CJS/ESM interop intact.
  resolve: {
    dedupe: ['svelte', '@tanstack/svelte-query', 'sv-router', '@lucide/svelte'],
    // The SDK ids must resolve for app files OUTSIDE the Vite root, where the
    // tsconfig-paths plugin doesn't reach ('/server' entry listed first — the
    // bare id would otherwise prefix-match it).
    alias: {
      '@talaria/sdk/server': resolve(here, 'src/sdk/server.ts'),
      '@talaria/sdk': resolve(here, 'src/sdk/index.ts'),
    },
  },
  plugins: [
    // loose: resolve `@/…` for non-TS files too (.svelte) — the default only
    // maps imports TypeScript itself would resolve.
    viteTsConfigPaths({ projects: ['./tsconfig.json'], loose: true }),
    tailwindcss(),
    svelte(),
    apiDev(),
  ],
})
