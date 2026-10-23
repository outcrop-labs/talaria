// The current build pointer decides which build exists: the door serves only
// the key it names (servableBuildKey), and once it names a ready build every
// other key directory of the app is pruned.
import { existsSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { pruneSupersededBuilds, servableBuildKey, writeCurrent, type AppBuildStatus } from './status'

const prev = process.env.TALARIA_APP_BUILDS_DIR
let root = ''
const app = () => join(root, 'builds', 'crm')

const point = (key: string, status: AppBuildStatus) =>
  writeCurrent('crm', { key, status, hostBuild: 'h', builtAt: 1, bytes: 1, files: ['app.js'] })

const build = (key: string) => {
  mkdirSync(join(app(), key, 'assets'), { recursive: true })
  writeFileSync(join(app(), key, 'app.js'), 'export default {}')
  writeFileSync(join(app(), key, 'assets', 'chunk.js'), 'x')
}

beforeEach(() => {
  root = mkdtempSync(join(tmpdir(), 'app-build-status-'))
  process.env.TALARIA_APP_BUILDS_DIR = join(root, 'builds')
  for (const key of ['0000000000000001', '0000000000000002', '0000000000000003']) build(key)
})
afterEach(() => {
  if (prev === undefined) delete process.env.TALARIA_APP_BUILDS_DIR
  else process.env.TALARIA_APP_BUILDS_DIR = prev
  rmSync(root, { recursive: true, force: true })
})

describe('servableBuildKey', () => {
  it('is the pointer\'s key once that build is ready, and nothing before', () => {
    expect(servableBuildKey('crm')).toBeNull()
    point('0000000000000002', 'building')
    expect(servableBuildKey('crm')).toBeNull()
    point('0000000000000002', 'failed')
    expect(servableBuildKey('crm')).toBeNull()
    point('0000000000000002', 'ready')
    expect(servableBuildKey('crm')).toBe('0000000000000002')
    // A pointer naming something that is not a key serves nothing.
    point('../other', 'ready')
    expect(servableBuildKey('crm')).toBeNull()
  })
})

describe('pruneSupersededBuilds', () => {
  it('removes every key directory but the ready one the pointer names', () => {
    point('0000000000000002', 'ready')
    expect(pruneSupersededBuilds('crm').sort()).toEqual(['0000000000000001', '0000000000000003'])
    expect(existsSync(join(app(), '0000000000000001'))).toBe(false)
    expect(existsSync(join(app(), '0000000000000003'))).toBe(false)
    expect(existsSync(join(app(), '0000000000000002', 'assets', 'chunk.js'))).toBe(true)
    expect(existsSync(join(app(), 'current.json'))).toBe(true)
    // Nothing left to prune.
    expect(pruneSupersededBuilds('crm')).toEqual([])
  })

  it('prunes nothing until the pointer names a ready build', () => {
    expect(pruneSupersededBuilds('crm')).toEqual([])
    for (const status of ['building', 'failed'] as const) {
      point('0000000000000002', status)
      expect(pruneSupersededBuilds('crm'), status).toEqual([])
    }
    for (const key of ['0000000000000001', '0000000000000002', '0000000000000003']) expect(existsSync(join(app(), key))).toBe(true)
  })

  it('touches only directories named like a key, and never follows a link', () => {
    point('0000000000000002', 'ready')
    writeFileSync(join(app(), 'current.json.tmp'), '{}')
    mkdirSync(join(app(), 'not-a-key'))
    writeFileSync(join(app(), 'aaaa'), 'a file named like a key')
    const outside = join(root, 'outside')
    mkdirSync(outside)
    writeFileSync(join(outside, 'keep.txt'), 'keep')
    symlinkSync(outside, join(app(), 'bbbb'))
    expect(pruneSupersededBuilds('crm').sort()).toEqual(['0000000000000001', '0000000000000003'])
    for (const name of ['current.json.tmp', 'not-a-key', 'aaaa', 'bbbb']) expect(existsSync(join(app(), name)), name).toBe(true)
    expect(existsSync(join(outside, 'keep.txt'))).toBe(true)
  })

  it('answers an app with no builds directory with nothing', () => {
    expect(pruneSupersededBuilds('nope')).toEqual([])
  })
})
