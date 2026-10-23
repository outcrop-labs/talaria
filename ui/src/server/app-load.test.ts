// loadAppHarnesses — the files under apps/<slug>/harnesses/, one loader each,
// from source in dev and from the build's harnesses.js in production.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { harnessesEntrySource, serverishFileNames } from './app-build/build'
import { appBuildKeyDir, harnessSourceFiles } from './app-build/paths'
import { writeCurrent } from './app-build/status'
import { appHasHarnesses, loadAppHarnesses, setSsrLoader } from './app-load'

let root = ''
const prev = { apps: process.env.TALARIA_APPS_DIR, builds: process.env.TALARIA_APP_BUILDS_DIR }

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'app-load-harnesses-'))
  process.env.TALARIA_APPS_DIR = join(root, 'apps')
  process.env.TALARIA_APP_BUILDS_DIR = join(root, 'app-builds')
  const dir = join(root, 'apps', 'support', 'harnesses')
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, 'triage.ts'), 'export default { id: "support:triage" }\n')
  writeFileSync(join(dir, 'archive.ts'), 'export default { id: "support:archive" }\n')
  writeFileSync(join(dir, 'triage.test.ts'), 'throw new Error("a test is not a harness")\n')
  writeFileSync(join(dir, 'types.d.ts'), 'export {}\n')
  writeFileSync(join(dir, 'README.md'), '# not a harness\n')
  mkdirSync(join(dir, 'nested'))
  mkdirSync(join(root, 'apps', 'plain'), { recursive: true })
})

afterAll(() => {
  setSsrLoader(null)
  if (prev.apps === undefined) delete process.env.TALARIA_APPS_DIR
  else process.env.TALARIA_APPS_DIR = prev.apps
  if (prev.builds === undefined) delete process.env.TALARIA_APP_BUILDS_DIR
  else process.env.TALARIA_APP_BUILDS_DIR = prev.builds
  rmSync(root, { recursive: true, force: true })
})

describe('the harness files an app ships', () => {
  it('are the .ts modules directly under harnesses/, sorted, never a test or a declaration', () => {
    expect(harnessSourceFiles(join(root, 'apps', 'support', 'harnesses'))).toEqual(['archive.ts', 'triage.ts'])
    expect(harnessSourceFiles(join(root, 'apps', 'plain', 'harnesses'))).toEqual([])
    expect(appHasHarnesses('support')).toBe(true)
    expect(appHasHarnesses('plain')).toBe(false)
  })

  it('load from source in dev, one loader per file', async () => {
    setSsrLoader((abs) => import(/* @vite-ignore */ abs))
    const files = await loadAppHarnesses('support')
    expect(files.map((f) => f.file)).toEqual(['archive.ts', 'triage.ts'])
    expect(await files[1]!.load()).toMatchObject({ default: { id: 'support:triage' } })
    setSsrLoader(null)
  })

  it("load from the build's harnesses.js in production, keeping only real entries", async () => {
    setSsrLoader(null)
    const key = 'abc123'
    const out = appBuildKeyDir('support', key)
    mkdirSync(join(out, '_harnesses'), { recursive: true })
    writeFileSync(join(out, '_harnesses', 'triage.js'), 'export default { id: "support:triage", built: true }\n')
    writeFileSync(
      join(out, 'harnesses.js'),
      `export const harnesses = {
  "triage.ts": () => import("./_harnesses/triage.js"),
  "../escape.ts": () => import("./_harnesses/triage.js"),
  "not-a-loader.ts": 42,
}
`,
    )
    writeCurrent('support', { key, status: 'ready', hostBuild: 'h', builtAt: 1, bytes: 1, files: ['harnesses.js'] })
    const files = await loadAppHarnesses('support')
    expect(files.map((f) => f.file)).toEqual(['triage.ts'])
    expect(await files[0]!.load()).toMatchObject({ default: { built: true } })
  })

  it('are nothing for a build that is not ready', async () => {
    setSsrLoader(null)
    writeCurrent('support', { key: 'abc123', status: 'failed', hostBuild: 'h', builtAt: 1, bytes: 1, files: ['harnesses.js'], error: 'x' })
    expect(await loadAppHarnesses('support')).toEqual([])
  })
})

describe('the harnesses.js build', () => {
  it('is an entry of lazy imports keyed by file name, one chunk each', () => {
    const src = harnessesEntrySource('/apps/support/harnesses', ['archive.ts', 'triage.ts'])
    expect(src).toBe(
      'export const harnesses = {\n' +
        '  "archive.ts": () => import("/apps/support/harnesses/archive.ts"),\n' +
        '  "triage.ts": () => import("/apps/support/harnesses/triage.ts"),\n' +
        '}\n',
    )
  })

  it('writes its chunks under _harnesses/, never beside the client', () => {
    expect(serverishFileNames('/b/k/harnesses.js')).toEqual({
      entryFileNames: 'harnesses.js',
      chunkFileNames: '_harnesses/[name]-[hash].js',
      assetFileNames: '_harnesses/[name]-[hash][extname]',
    })
    expect(serverishFileNames('/b/k/server.js').entryFileNames).toBe('server.js')
    expect(serverishFileNames('/b/k/mcp.js').chunkFileNames).toBe('_mcp/[name]-[hash].js')
  })
})
