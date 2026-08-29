// `validateEnv` is a boot gate: server-entry.js calls it and exits 1 on a throw.
// That makes a false rejection an outage, so what is tested here is mostly the
// ACCEPT side — the shapes a working deployment really has (a bare
// `postgres://`, a URL with query params, AUTH_SECRET alone as the encryption
// root, NODE_ENV unset) must all survive. The reject cases are the three the
// audit called boot-blocking.
import { describe, it, expect, vi, afterEach } from 'vitest'
import { validateEnv, DEV_S3_SECRET } from './env'

/** A minimally-valid environment; each test bends one thing. */
const base = (over: Record<string, string | undefined> = {}): NodeJS.ProcessEnv => ({
  DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5544/talaria',
  REDIS_URL: 'redis://127.0.0.1:6399',
  TALARIA_SECRET_KEY: 'a-root-secret',
  ...over,
})

afterEach(() => vi.restoreAllMocks())

/** collectWarnings writes to console.warn; silence it and hand back the calls. */
const captureWarnings = () => vi.spyOn(console, 'warn').mockImplementation(() => {})

describe('validateEnv — accepts what real deployments look like', () => {
  it('accepts the minimal valid environment', () => {
    captureWarnings()
    expect(() => validateEnv(base())).not.toThrow()
  })

  it('accepts postgresql:// as well as postgres://', () => {
    captureWarnings()
    expect(() => validateEnv(base({ DATABASE_URL: 'postgresql://u:p@db:5432/t' }))).not.toThrow()
  })

  it('accepts rediss:// (TLS)', () => {
    captureWarnings()
    expect(() => validateEnv(base({ REDIS_URL: 'rediss://cache.example.com:6380' }))).not.toThrow()
  })

  it('accepts a connection string carrying query params', () => {
    captureWarnings()
    expect(() =>
      validateEnv(base({ DATABASE_URL: 'postgres://u:p@db:5432/t?sslmode=require&pool_max=10' })),
    ).not.toThrow()
  })

  it('accepts AUTH_SECRET alone as the encryption root, but warns about it', () => {
    const warn = captureWarnings()
    expect(() =>
      validateEnv(base({ TALARIA_SECRET_KEY: undefined, AUTH_SECRET: 'doing-double-duty' })),
    ).not.toThrow()
    expect(warn.mock.calls.flat().join('\n')).toMatch(/double duty/i)
  })

  it('accepts NODE_ENV unset (every consumer reads unset as development)', () => {
    captureWarnings()
    expect(() => validateEnv(base({ NODE_ENV: undefined }))).not.toThrow()
  })

  it('ignores environment variables it does not name', () => {
    captureWarnings()
    expect(() => validateEnv(base({ SOME_UNRELATED_THING: 'x', TZ: 'UTC' }))).not.toThrow()
  })

  it('treats blank as unset rather than as a bad value', () => {
    captureWarnings()
    // An empty PORT= line in .env must not read as "port zero".
    expect(() => validateEnv(base({ PORT: '' }))).not.toThrow()
  })
})

describe('validateEnv — refuses the boot-blocking cases', () => {
  it('refuses a missing DATABASE_URL', () => {
    captureWarnings()
    expect(() => validateEnv(base({ DATABASE_URL: undefined }))).toThrow(/DATABASE_URL/)
  })

  it('refuses a DATABASE_URL that is not a postgres connection string', () => {
    captureWarnings()
    expect(() => validateEnv(base({ DATABASE_URL: 'mysql://u:p@db/t' }))).toThrow(/postgres/)
  })

  it('refuses a missing REDIS_URL', () => {
    captureWarnings()
    expect(() => validateEnv(base({ REDIS_URL: undefined }))).toThrow(/REDIS_URL/)
  })

  it('refuses an instance with no encryption root at all', () => {
    captureWarnings()
    expect(() =>
      validateEnv(base({ TALARIA_SECRET_KEY: undefined, AUTH_SECRET: undefined })),
    ).toThrow(/encryption root is unset/)
  })

  it('refuses a NODE_ENV typo — the case that silently drops Secure cookies', () => {
    captureWarnings()
    expect(() => validateEnv(base({ NODE_ENV: 'prod' }))).toThrow(/NODE_ENV/)
  })

  it('refuses the published dev S3 password in production', () => {
    captureWarnings()
    expect(() =>
      validateEnv(base({ NODE_ENV: 'production', TALARIA_S3_SECRET_KEY: DEV_S3_SECRET })),
    ).toThrow(/published dev default/)
  })

  it('allows the dev S3 password outside production', () => {
    captureWarnings()
    expect(() =>
      validateEnv(base({ NODE_ENV: 'development', TALARIA_S3_SECRET_KEY: DEV_S3_SECRET })),
    ).not.toThrow()
  })

  it('reports every problem in one throw, not just the first', () => {
    captureWarnings()
    try {
      validateEnv(base({ DATABASE_URL: undefined, REDIS_URL: undefined }))
      expect.unreachable('should have thrown')
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      expect(message).toMatch(/DATABASE_URL/)
      expect(message).toMatch(/REDIS_URL/)
      expect(message).toMatch(/2 problems/)
    }
  })

  it('names the file it could not read for TALARIA_SECRET_KEY_FILE', () => {
    captureWarnings()
    expect(() =>
      validateEnv(base({ TALARIA_SECRET_KEY: undefined, TALARIA_SECRET_KEY_FILE: '/nonexistent/talaria.key' })),
    ).toThrow(/nonexistent\/talaria\.key/)
  })
})

describe('validateEnv — warnings describe an instance that still runs', () => {
  // A fresh instance with zero admins is NOT a warning anymore — that is the
  // claim state, surfaced by /claim and the login screen, not by the boot
  // (env.ts cannot see the database).

  it('stays quiet when the environment is fully configured', () => {
    const warn = captureWarnings()
    validateEnv(base())
    expect(warn).not.toHaveBeenCalled()
  })
})
