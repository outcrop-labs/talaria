import { defineConfig } from 'vitest/config'
import viteTsConfigPaths from 'vite-tsconfig-paths'

// Deliberately minimal, and deliberately NOT `vite.config.ts`: the app config
// pulls in the Svelte plugin, the server build and Tailwind, none of which a
// unit test needs (and all of which make the run slow and order-dependent).
// The one thing we do reuse is the tsconfig path mapping, so `@/…` resolves in
// tests exactly as it does in the app.
export default defineConfig({
  plugins: [viteTsConfigPaths({ projects: ['./tsconfig.json'] })],
  test: {
    // Every module under test is server-side or pure — no DOM needed.
    environment: 'node',
    include: ['src/**/*.test.ts'],
    // `src/routes/**` is FILE-BASED ROUTING, where a dot is a path separator:
    // `src/routes/api/mcp.test.ts` is the handler for POST /api/mcp/test, NOT a
    // test file. Nothing under routes/ is a test, and this exclusion is what
    // stops vitest importing a route module and executing it as a suite.
    //
    // THE COST, AND IT IS A REAL ONE: nothing under routes/ can be unit tested
    // at all. Route-level behavior therefore has no home here — `routes/api/
    // muse.ts`'s streaming and the grounded redact in
    // `routes/api/llm.v1.chat.completions.ts` are both logic this project
    // changed and that no test in this config can reach.
    //
    // THE FIX IS NOT TO LOOSEN THIS EXCLUSION, because the collision is
    // structural: any pattern that admits `routes/api/foo.test.ts` as a test
    // also admits every route whose path segment happens to be `test`, and
    // importing a route module executes it. Keep route files thin — parse the
    // request, call one function in `src/server/*`, serialize the result — and
    // test that function, which is what every route this project touched now
    // does (`api/muse.ts` calls `runHarnessStreamed`, `api/llm.v1.chat.
    // completions.ts` calls `guardCompletion` + `redactSecrets`, and both of
    // those ARE covered). What is left over is request parsing and status
    // codes, and the honest tool for that is an end-to-end run against a live
    // server, which this repo does not have a home for yet.
    exclude: ['src/routes/**', 'node_modules/**'],
    // Tests must be self-contained: no service, no network, no clock games.
    testTimeout: 10_000,
  },
})
