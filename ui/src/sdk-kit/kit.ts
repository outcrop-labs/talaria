// THE SDK KIT — what an instance hands a plugin repo that lives outside it.
//
// @talaria/sdk resolves only inside a host checkout (a tsconfig path alias
// into ui/src/sdk), so a Developer Agent writing an app in its own repo, in
// its own sandbox, could not typecheck it, build it or run its evals: the
// first compile was the host's, after a person merged and an admin installed,
// and a failure there disabled the app. The kit closes that. The instance
// serves it (`GET /api/sdk/kit.tgz`), built from the same host build as the
// instance itself, and it carries:
//
//   types/            declarations for @talaria/sdk, /server and /testing
//   lib/, dist/       the host's own build recipe, load probe and harness
//                     runner, bundled to run with no host and no database
//   src/styles.css    the host's stylesheet, which an app's client CSS is
//   svelte.config.ts  compiled against, and the svelte config
//   schema/           talaria.json's published schema
//   bin/talaria-app   scaffold | check | build | eval
//   kit.json          this file's `KitManifest`: the version, the SDK
//                     version, and the shared-module lists
//
// Nothing in it is a secret. Everything it runs runs in the sandbox that
// fetched it: the host's load probe imports app code into its own process,
// so unmerged code is never checked there.
//
// This module is the vocabulary both halves share — the builder
// (ui/scripts/build-sdk-kit.ts), the route that serves it and the CLI.

/** The kit's own layout version. A CLI reading a kit.json with another
 *  format refuses it rather than guessing at its paths. */
export const KIT_FORMAT = 1

/** The header the kit route answers with, beside `etag`: the kit's version.
 *  prepare_env installs by it. */
export const KIT_VERSION_HEADER = 'x-talaria-kit-version'

/** A kit version: the host build stamp, then a digest of the kit's own
 *  contents (`dev-a962c3625153-1f2e3d4c`). The stamp alone does not move when
 *  the SDK does — it hashes the shared-module list and the svelte version —
 *  and a sandbox installs one directory per version, so the contents are in
 *  the name. It names that directory, so it is held to a path-safe shape. */
export const KIT_VERSION_RE = /^[A-Za-z0-9][A-Za-z0-9._+-]{0,79}$/

export const kitVersionOk = (v: string): boolean => KIT_VERSION_RE.test(v)

/** kit.json. */
export interface KitManifest {
  format: typeof KIT_FORMAT
  /** `<hostBuild>-<contents digest>`. */
  version: string
  /** The host build the kit came from — the same stamp the host's app builds
   *  are keyed on, so a kit build of a tree lands on the host's build key. */
  hostBuild: string
  /** The @talaria/sdk version this host serves (`SDK_VERSION`), the one a
   *  talaria.json `sdk` range is checked against. */
  sdk: string
  /** The shared modules — what an app imports from the host and never
   *  bundles (app-runtime/specifiers.ts). A bare import outside these lists
   *  does not resolve for an app, on the host or here. */
  specifiers: { client: string[]; server: string[] }
  /** Exact versions of the packages the kit installs, read off the host's own
   *  node_modules: the svelte compiler an app is built with here is the one
   *  the host builds it with. */
  packages: Record<string, string>
}

/** The packages the kit installs into its own node_modules (never the app's):
 *  the build toolchain the host's recipe uses, the checkers, and the
 *  packages the SDK's declarations name. */
export const KIT_PACKAGES: readonly string[] = [
  // the build recipe (app-build/build.ts)
  'vite',
  'svelte',
  '@sveltejs/vite-plugin-svelte',
  'tailwindcss',
  '@tailwindcss/vite',
  // check
  'typescript',
  'svelte-check',
  'vitest',
  '@types/node',
  // what the SDK's declarations, and an app's client code, import by name
  '@tanstack/svelte-query',
  '@lucide/svelte',
  'zod',
  'clsx',
  '@tiptap/core',
  'svelte-tiptap',
]

/** The shared client packages an app imports by name, whole or by a listed
 *  subpath: what a typecheck must resolve from the kit, and nothing else. */
export const SHARED_TYPE_PACKAGES: readonly string[] = ['svelte', '@lucide/svelte', '@tanstack/svelte-query']

/** The kit-relative paths everything else reads. */
export const KIT_PATHS = {
  manifest: 'kit.json',
  cli: 'bin/talaria-app',
  schema: 'schema/talaria.schema.json',
  types: {
    client: 'types/src/sdk/index.d.ts',
    server: 'types/src/sdk/server.d.ts',
    testing: 'types/src/sdk/testing.d.ts',
  },
  runtime: {
    /** The `@talaria/sdk/server` shim an app's server, mcp and harness
     *  chunks link to — where the host's build recipe looks for it
     *  (`dist/server/runtime/rt-talaria-sdk-server.js` under its root). */
    server: 'dist/server/runtime/rt-talaria-sdk-server.js',
    testing: 'lib/sdk-testing.js',
    /** What a test that imports the client SDK gets: a module that says the
     *  client does not run under the kit's tests. */
    client: 'lib/sdk-client.js',
  },
  /** The host build stamp, where the build recipe reads it. */
  buildId: 'dist/client/runtime/build-id',
} as const
