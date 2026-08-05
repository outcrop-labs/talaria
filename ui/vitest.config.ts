import { defineConfig } from 'vitest/config'
import viteTsConfigPaths from 'vite-tsconfig-paths'

// Deliberately minimal, and deliberately NOT `vite.config.ts`: the app config
// pulls in TanStack Start, the router codegen plugin and Tailwind, none of which
// a unit test needs (and all of which make the run slow and order-dependent).
// The one thing we do reuse is the tsconfig path mapping, so `@/…` resolves in
// tests exactly as it does in the app.
export default defineConfig({
  plugins: [viteTsConfigPaths({ projects: ['./tsconfig.json'] })],
  test: {
    // Every module under test is server-side or pure — no DOM needed.
    environment: 'node',
    include: ['src/**/*.test.ts'],
    // `src/routes/**` is TanStack file-based routing, where a dot is a path
    // separator: `src/routes/api/mcp.test.ts` is the handler for
    // POST /api/mcp/test, NOT a test file. Nothing under routes/ is a test.
    exclude: ['src/routes/**', 'node_modules/**'],
    // Tests must be self-contained: no service, no network, no clock games.
    testTimeout: 10_000,
  },
})
