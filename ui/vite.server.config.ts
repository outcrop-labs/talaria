import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import viteTsConfigPaths from 'vite-tsconfig-paths'
import { appRuntimeHost } from './src/app-runtime/vite-plugin'

const here = fileURLToPath(new URL('.', import.meta.url))

// Server bundle: src/server/app.ts → dist/server/server.js, the fetch handler
// server-entry.ts imports. Extra `runtime/rt-*` entries are the stable
// specifiers independently-built app server/mcp chunks import, so they share
// the host's SDK instance. Node builtins and node_modules deps stay external.
export default defineConfig({
  resolve: {
    alias: {
      '@talaria/sdk/server': resolve(here, 'src/sdk/server.ts'),
      '@talaria/sdk': resolve(here, 'src/sdk/index.ts'),
    },
  },
  plugins: [viteTsConfigPaths({ projects: ['./tsconfig.json'], loose: true }), appRuntimeHost({ ssr: true })],
  build: {
    ssr: true,
    outDir: 'dist/server',
    emptyOutDir: true,
    target: 'node22',
    rollupOptions: {
      input: { server: resolve(here, 'src/server/app.ts') },
      output: {
        entryFileNames: (c) => (c.name === 'server' ? 'server.js' : '[name].js'),
      },
    },
  },
})
