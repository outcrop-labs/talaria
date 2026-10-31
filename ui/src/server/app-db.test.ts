import { mkdtempSync, readFileSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  appDataDir,
  appDbContainer,
  appDbUrl,
  composeYaml,
  dialsByName,
  inImage,
  legacyAppDbNetwork,
  occupantPlan,
  occupantsOf,
  postgresAliasNetworks,
  writeDbEnv,
} from './app-db'

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

  // A compose file that names no network gets `<project>_default` made for
  // it: a subnet per app DB from docker's ~30 address pools (R19). The host
  // dials the published loopback port, so the default bridge serves.
  it('puts a loopback-published DB on the default bridge, so compose makes no network', () => {
    const y = composeYaml({ image: 'postgres:16-alpine', container: 'talaria-appdb-x-contacts', network: null })
    expect(y).toContain('    network_mode: bridge\n')
    expect(y).not.toMatch(/^networks:/m)
    expect(y).not.toMatch(/^ {4}networks:/m)
    // the port is still published, on loopback only
    expect(y.indexOf('network_mode: bridge')).toBeLessThan(y.indexOf('ports:'))
    expect(y).toContain('- "127.0.0.1::5432"')
  })

  it('attaches an external network instead of publishing a port', () => {
    const y = composeYaml({
      image: 'postgres:16-alpine',
      container: 'talaria-appdb-x-contacts',
      network: 'foo_internal',
    })
    expect(y).toContain('name: foo_internal')
    expect(y).toContain('external: true')
    expect(y).not.toContain('ports:')
    expect(y).not.toContain('network_mode')
  })

  // Compose aliases a service by its name on every network it joins. On a
  // named network the platform's database is `postgres`, so an app DB named
  // that answered for it too.
  it('names the service for its container, so no network gets a second `postgres`', () => {
    for (const network of [null, 'devbox-demo_default', 'foo_internal']) {
      const y = composeYaml({ image: 'postgres:16-alpine', container: 'talaria-appdb-x-contacts', network })
      expect(y).toMatch(/^services:\n {2}talaria-appdb-x-contacts:\n/m)
      expect(y).not.toMatch(/^ {2}postgres:/m)
    }
  })

  it('names the network compose used to make for an app DB', () => {
    expect(legacyAppDbNetwork('talaria-appdb-x-contacts')).toBe('talaria-appdb-x-contacts_default')
  })
})

// An app DB made before its compose service was named for its container is
// still `postgres` on the networks it joined, and keeps running across
// reboots, so appSql recreates it rather than reusing it.
describe('postgresAliasNetworks', () => {
  it('finds a named network where the container answers to `postgres`', () => {
    expect(
      postgresAliasNetworks({
        talaria_internal: {
          Aliases: ['talaria-appdb-talaria-crm', 'postgres'],
          DNSNames: ['talaria-appdb-talaria-crm', 'postgres', '0f1e2d3c4b5a'],
        },
      }),
    ).toEqual(['talaria_internal'])
  })

  it('passes one dialled only by its own name', () => {
    expect(
      postgresAliasNetworks({
        talaria_internal: { Aliases: ['talaria-appdb-talaria-crm', 'talaria-appdb-talaria-crm'], DNSNames: null },
      }),
    ).toEqual([])
  })

  // The default bridge has no DNS: a host-run app's DB there answers nothing,
  // so it is reused as before.
  it('ignores the default bridge, and a container with no networks', () => {
    expect(postgresAliasNetworks({ bridge: { Aliases: ['postgres'], DNSNames: null } })).toEqual([])
    expect(postgresAliasNetworks({ bridge: null })).toEqual([])
    expect(postgresAliasNetworks(null)).toEqual([])
  })
})

describe('the other containers on an app DB\'s cluster', () => {
  const pg = '/srv/devboxes/demo/state/app-data/crm/pg'
  const line = (name: string, running: boolean, sources: string[]) =>
    `/${name}\t${running}\t${JSON.stringify(sources.map((Source) => ({ Type: 'bind', Source, Destination: '/var/lib/postgresql/data' })))}`

  it('reads them by mount source, not by name', () => {
    const out = [
      line('talaria-appdb-1-crm', true, [pg]),
      line('talaria-appdb-1-notes', true, ['/srv/devboxes/demo/state/app-data/notes/pg']),
      line('talaria-appdb-demo-b-crm', false, ['/srv/devboxes/demo-b/state/app-data/crm/pg']),
      line('talaria-appdb-x-crm', false, [`${pg}/`]),
      'not a line',
    ].join('\n')
    expect(occupantsOf(out, pg)).toEqual([
      { name: 'talaria-appdb-1-crm', running: true },
      { name: 'talaria-appdb-x-crm', running: false },
    ])
  })

  // A box's instance was `1` before it was the box's name: its old database
  // is on the same data, and two postmasters there corrupt the cluster.
  it("retires a box's database under its old name, running or not", () => {
    expect(occupantPlan([{ name: 'talaria-appdb-1-crm', running: true }], 'demo')).toEqual({
      retire: ['talaria-appdb-1-crm'],
      refuse: null,
    })
    expect(occupantPlan([{ name: 'talaria-appdb-1-crm', running: false }], 'demo')).toEqual({
      retire: ['talaria-appdb-1-crm'],
      refuse: null,
    })
  })

  // Not a box, or a box that still says `1`: the old name is not this
  // process's to stop.
  it('refuses any other running one, and leaves a stopped one', () => {
    expect(occupantPlan([{ name: 'talaria-appdb-talaria-crm', running: true }], undefined)).toEqual({
      retire: [],
      refuse: 'talaria-appdb-talaria-crm',
    })
    expect(occupantPlan([{ name: 'talaria-appdb-1-crm', running: true }], undefined).refuse).toBe('talaria-appdb-1-crm')
    expect(occupantPlan([{ name: 'talaria-appdb-1-crm', running: true }], '1').refuse).toBe('talaria-appdb-1-crm')
    expect(occupantPlan([{ name: 'talaria-appdb-main-crm', running: false }], 'demo')).toEqual({ retire: [], refuse: null })
    expect(occupantPlan([], 'demo')).toEqual({ retire: [], refuse: null })
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
