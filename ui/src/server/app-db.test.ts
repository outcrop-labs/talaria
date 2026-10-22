import { mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { appDataDir, appDbContainer, appDbUrl, composeYaml, dialsByName, inImage, writeDbEnv } from './app-db'

describe('app db identity', () => {
  it('names containers from the instance + slug', () => {
    const prev = process.env.TALARIA_WORKTREE
    process.env.TALARIA_WORKTREE = 'apps-runtime'
    expect(appDbContainer('contacts')).toBe('talaria-appdb-apps-runtime-contacts')
    if (prev === undefined) delete process.env.TALARIA_WORKTREE
    else process.env.TALARIA_WORKTREE = prev
  })

  it('builds a postgres URL with the given password encoded', () => {
    const url = appDbUrl('contacts', 5432, '127.0.0.1', 'p@ss/word')
    expect(url).toBe('postgres://talaria:p%40ss%2Fword@127.0.0.1:5432/talaria')
    expect(url).not.toContain('p@ss/word')
  })
})

describe('composeYaml', () => {
  it('keeps the password out of the compose file', () => {
    const y = composeYaml({
      image: 'docker.io/library/postgres:16-alpine@sha256:abc',
      container: 'talaria-appdb-x-contacts',
      network: null,
    })
    expect(y).toContain('env_file:')
    expect(y).toContain('db.env')
    expect(y).toContain('127.0.0.1::5432')
    expect(y).toContain('postgres:16-alpine@sha256:abc')
    expect(y).not.toMatch(/PASSWORD/i)
    expect(y).not.toMatch(/secret/i)
  })

  it('attaches an external network instead of publishing a port', () => {
    const y = composeYaml({
      image: 'postgres:16-alpine',
      container: 'talaria-appdb-x-contacts',
      network: 'foo_internal',
    })
    expect(y).toContain('name: foo_internal')
    expect(y).not.toContain('ports:')
  })
})

describe('writeDbEnv', () => {
  it("creates a fresh app's data directory before writing db.env", () => {
    const root = mkdtempSync(join(tmpdir(), 'app-data-'))
    const prev = process.env.TALARIA_APP_DATA_DIR
    process.env.TALARIA_APP_DATA_DIR = join(root, 'app-data')
    try {
      writeDbEnv('fresh', 's3cret')
      const dir = appDataDir('fresh')
      expect(statSync(dir).isDirectory()).toBe(true)
      expect(statSync(dir).mode & 0o777).toBe(0o700)
      const file = join(dir, 'db.env')
      expect(statSync(file).mode & 0o777).toBe(0o600)
      expect(readFileSync(file, 'utf8')).toContain('POSTGRES_PASSWORD=s3cret\n')
      writeDbEnv('fresh', 'rotated')
      expect(readFileSync(file, 'utf8')).toContain('POSTGRES_PASSWORD=rotated\n')
    } finally {
      if (prev === undefined) delete process.env.TALARIA_APP_DATA_DIR
      else process.env.TALARIA_APP_DATA_DIR = prev
      rmSync(root, { recursive: true, force: true })
    }
  })
})

describe('inImage', () => {
  const withEnv = (env: Record<string, string | undefined>, fn: () => void) => {
    const prev = Object.fromEntries(Object.keys(env).map((k) => [k, process.env[k]]))
    for (const [k, v] of Object.entries(env)) {
      if (v === undefined) delete process.env[k]
      else process.env[k] = v
    }
    try {
      fn()
    } finally {
      for (const [k, v] of Object.entries(prev)) {
        if (v === undefined) delete process.env[k]
        else process.env[k] = v
      }
    }
  }

  it('is the image only when the image says so', () => {
    withEnv({ TALARIA_INSTALL: 'image', TALARIA_RUNTIME: 'prod-server' }, () => expect(inImage()).toBe(true))
  })

  it('treats a checkout install under server-entry.ts as the host, not the image', () => {
    withEnv({ TALARIA_INSTALL: undefined, TALARIA_RUNTIME: 'prod-server' }, () => expect(inImage()).toBe(false))
    withEnv({ TALARIA_INSTALL: undefined, TALARIA_RUNTIME: undefined }, () => expect(inImage()).toBe(false))
  })

  // TALARIA_APP_DB_NETWORK attaches the DB to that network with no published
  // port, so a host outside the image that sets it must dial by name too.
  it('dials by name in the image or on a named app DB network, else the loopback port', () => {
    const host = { TALARIA_INSTALL: undefined, TALARIA_RUNTIME: 'prod-server' }
    withEnv({ ...host, TALARIA_APP_DB_NETWORK: 'custom_internal' }, () => expect(dialsByName()).toBe(true))
    withEnv({ ...host, TALARIA_APP_DB_NETWORK: undefined }, () => expect(dialsByName()).toBe(false))
    withEnv({ TALARIA_INSTALL: 'image', TALARIA_APP_DB_NETWORK: undefined }, () => expect(dialsByName()).toBe(true))
  })
})
