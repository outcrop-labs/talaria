import { afterEach, beforeEach, describe, expect, it } from 'vitest'

// internalTarget()'s use-time guard — the half of the contract server/env.ts's
// boot validation promises ("the built-in storage mode will refuse to run
// rather than fall back to the published dev password"). Nothing here touches
// a bucket: no network, no app_settings row, just the target the mode WOULD
// credential with.
import { DEV_S3_SECRET } from './env'
import { refuseDevSecret, type BucketTarget } from './storage'

const targetWith = (secretAccessKey: string): BucketTarget => ({
  endpoint: 'http://127.0.0.1:9010',
  region: 'us-east-1',
  bucket: 'talaria',
  accessKeyId: 'talaria',
  secretAccessKey,
  pathStyle: true,
  prefix: '',
})

const KEYS = ['NODE_ENV', 'TALARIA_S3_SECRET_KEY'] as const
let saved: Record<string, string | undefined>

beforeEach(() => {
  saved = Object.fromEntries(KEYS.map((k) => [k, process.env[k]]))
  delete process.env.TALARIA_S3_SECRET_KEY
  delete process.env.NODE_ENV
})

afterEach(() => {
  for (const k of KEYS) {
    if (saved[k] === undefined) delete process.env[k]
    else process.env[k] = saved[k]
  }
})

describe('refuseDevSecret', () => {
  it('refuses the dev fallback in production', () => {
    process.env.NODE_ENV = 'production'
    expect(() => refuseDevSecret(targetWith(DEV_S3_SECRET))).toThrow(/TALARIA_S3_SECRET_KEY is unset/)
  })

  it('allows a real secret in production', () => {
    process.env.NODE_ENV = 'production'
    expect(() => refuseDevSecret(targetWith('openssl-rand-hex-24'))).not.toThrow()
  })

  it('allows the dev fallback outside production — local dev runs on it', () => {
    process.env.NODE_ENV = 'development'
    expect(() => refuseDevSecret(targetWith(DEV_S3_SECRET))).not.toThrow()
  })
})
