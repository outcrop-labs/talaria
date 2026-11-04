// `// @vitest-environment runes` — a test that needs Svelte's CLIENT runtime,
// in Node, with no DOM.
//
// The suite's `node` environment compiles every `*.svelte.ts` module (and
// svelte-query's own `.svelte.js`) for the SERVER, where `$effect` is a no-op
// and `$effect.root` never runs its body: a store's reactivity cannot be
// observed there at all. This environment asks Vite for the client transform
// (`viteEnvironment: 'client'`), so `$derived`, `$effect` and `flushSync`
// behave as they do in the app. It installs no DOM, so nothing here can mount
// a component; it is for testing what a runes module promises its callers.
//
// Vitest looks the name up as the package `vitest-environment-runes`, which
// vitest.config.ts aliases to this file.
export default {
  name: 'runes',
  viteEnvironment: 'client',
  setup() {
    return { teardown() {} }
  },
}
