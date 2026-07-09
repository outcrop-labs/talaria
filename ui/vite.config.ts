import { existsSync, statSync } from 'node:fs'
import { tanstackStart } from '@tanstack/react-start/plugin/vite'
import viteReact from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'
import viteTsConfigPaths from 'vite-tsconfig-paths'

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
  server: { host: true, allowedHosts: true },
  plugins: [
    viteTsConfigPaths({ projects: ['./tsconfig.json'] }),
    tailwindcss(),
    tanstackStart(),
    viteReact(),
  ],
})
