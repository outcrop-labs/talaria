import { defineConfig } from 'vitest/config'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import viteTsConfigPaths from 'vite-tsconfig-paths'

// Deliberately minimal, and deliberately NOT `vite.config.ts`: the app config
// pulls in the server build and Tailwind, none of which a unit test needs
// (and all of which make the run slow and order-dependent). The Svelte plugin
// IS included now, for one reason: module-level reactive stores
// (`*.svelte.ts` — agent-design, toast, boards) use `$state`, and the store
// under test must compile to real reactivity the same way it does in the app.
// Its runtime pieces (HMR, dev websocket) are inert here — no dev server, no
// browser — so the plugin is transform-only in practice. The tsconfig path
// mapping is reused so `@/…` resolves in tests exactly as it does in the app.
export default defineConfig({
  plugins: [
    viteTsConfigPaths({ projects: ['./tsconfig.json'] }),
    svelte({
      compilerOptions: {
        // Runes modules ($state in *.svelte.ts) compile in runes mode by
        // file extension; no component is loaded by these tests and none of
        // the browser-env assumptions (document, window) can be reached.
        runes: true,
      },
    }),
  ],
  test: {
    // Modules under test are server-side, pure, or runes stores — no DOM.
    environment: 'node',
    include: ['src/**/*.test.ts'],
    // `src/routes/**` is FILE-BASED ROUTING, where a dot is a path separator:
    // `src/routes/api/mcp.test.ts` is the handler for POST /api/mcp/test, NOT a
    // test file. Nothing under routes/ is a test, and this exclusion is what
    // stops vitest importing a route module and executing it as a suite.
    //
    // THE COST, AND IT IS A REAL ONE: nothing under routes/ can be unit tested
    // at all — and since the cutover that is where the whole /api/* surface
    // lives, in Rust (api/src/routes/), covered by cargo tests rather than
    // here. The four TS residents this exclusion still guards are dispatch
    // thin by construction.
    //
    // THE FIX IS NOT TO LOOSEN THIS EXCLUSION, because the collision is
    // structural: any pattern that admits `routes/api/foo.test.ts` as a test
    // also admits every route whose path segment happens to be `test`, and
    // importing a route module executes it. Keep route files thin — parse the
    // request, call one function, serialize the request, and the honest tool
    // for what's left is an end-to-end run against a live server, which this
    // repo does not have a home for yet.
    exclude: ['src/routes/**', 'node_modules/**'],
    // Tests must be self-contained: no service, no network, no clock games.
    testTimeout: 10_000,
  },
})
