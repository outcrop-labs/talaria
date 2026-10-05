// Bundles the bridge into one self-contained file the api can spawn.
//
// The bundle is the deliverable, not the node_modules tree: `@oh-my-pi/pi-ai`
// resolves `@oh-my-pi/pi-natives` to 364 MB of glibc-only prebuilt binaries
// that this image (alpine) cannot load and no OAuth flow needs, so the
// bundler substitutes src/natives-stub.ts for it and everything else is
// inlined. See that file for which symbols are stubbed and why each one is
// safe. Nothing is patched: upstream's own flow code runs, verbatim, from
// whatever version package.json pins.

import { rm } from 'node:fs/promises'

const root = new URL('.', import.meta.url).pathname
const stub = `${root}src/natives-stub.ts`

await rm(`${root}dist`, { recursive: true, force: true })

const result = await Bun.build({
  entrypoints: [`${root}src/server.ts`],
  outdir: `${root}dist`,
  target: 'bun',
  // One file: the api spawns dist/server.js with no install beside it.
  splitting: false,
  plugins: [
    {
      name: 'stub-pi-natives',
      setup(build) {
        build.onResolve({ filter: /^@oh-my-pi\/pi-natives($|\/)/ }, () => ({ path: stub }))
      },
    },
  ],
})

if (!result.success) {
  for (const log of result.logs) console.error(String(log))
  process.exit(1)
}

for (const out of result.outputs) {
  const bytes = (await out.arrayBuffer()).byteLength
  console.log(`omp-auth: ${out.path.replace(root, '')} (${(bytes / 1e6).toFixed(1)} MB)`)
}
