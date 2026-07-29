import { existsSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { tanstackStart } from '@tanstack/react-start/plugin/vite'
import viteReact from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'
import viteTsConfigPaths from 'vite-tsconfig-paths'

const here = fileURLToPath(new URL('.', import.meta.url))

// Talaria UI — Vite + TanStack Start (matches the hermes-workspace stack so its
// chat/agent components lift with minimal friction). Tailwind v4 via the vite
// plugin; path alias `@/*` → `src/*` (see tsconfig).
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
  // App codebases have no node_modules of their own; shared deps resolve from
  // the host's — the peer-dependency model (one React, one router, one query
  // client across the whole deployment). dedupe (not alias) keeps Vite's
  // normal CJS/ESM interop and SSR externalization intact.
  resolve: {
    dedupe: ['react', 'react-dom', '@tanstack/react-query', '@tanstack/react-router', 'lucide-react'],
    // The SDK ids must resolve for app files OUTSIDE the Vite root, where the
    // tsconfig-paths plugin doesn't reach ('/server' entry listed first — the
    // bare id would otherwise prefix-match it).
    alias: {
      '@talaria/sdk/server': resolve(here, 'src/sdk/server.ts'),
      '@talaria/sdk': resolve(here, 'src/sdk/index.ts'),
    },
  },
  plugins: [
    viteTsConfigPaths({ projects: ['./tsconfig.json'] }),
    tailwindcss(),
    tanstackStart(),
    viteReact(),
  ],
})
