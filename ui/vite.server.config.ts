import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import viteTsConfigPaths from 'vite-tsconfig-paths'

const here = fileURLToPath(new URL('.', import.meta.url))

// Server bundle: src/server/app.ts → dist/server/server.js, the fetch handler
// server-entry.js imports. Node builtins and node_modules deps stay external
// (default SSR behaviour) — this bundle is run in place, not shipped alone.
export default defineConfig({
  resolve: {
    alias: {
      '@talaria/sdk/server': resolve(here, 'src/sdk/server.ts'),
      '@talaria/sdk': resolve(here, 'src/sdk/index.ts'),
    },
  },
  plugins: [viteTsConfigPaths({ projects: ['./tsconfig.json'], loose: true })],
  build: {
    ssr: 'src/server/app.ts',
    outDir: 'dist/server',
    emptyOutDir: true,
    target: 'node22',
    rollupOptions: {
      output: { entryFileNames: 'server.js' },
    },
  },
})
