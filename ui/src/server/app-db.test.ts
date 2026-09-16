import { describe, expect, it } from 'vitest'
import { appDbContainer, appDbUrl, composeYaml } from './app-db'

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
