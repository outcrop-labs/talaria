// Container readiness gate — the entrypoint runs this before exec'ing the app.
//
// Migrations run lazily on the first query (ui/src/server/db/pg.ts
// ensureMigrated, advisory-locked), and while a failed attempt self-heals on
// the next query, a boot that races Postgres spends its first minutes logging
// scary connection errors. The golden-image unit gated on pg_isready for the
// same reason; here the probe uses the app's own drivers, so the image needs
// no postgres client.
//
// Resolves deps through the app's node_modules via createRequire — this file
// lives in docker/, one level above them.
import { createRequire } from 'node:module'

const require = createRequire('/app/ui/package.json')

const DEADLINE_MS = 120_000
const RETRY_MS = 2_000
const started = Date.now()

async function pgReady() {
  // postgres is ESM-with-default under bun's require: the namespace object,
  // not the callable. (ioredis below is classic CJS — a function as-is.)
  const sql = require('postgres').default ?? require('postgres')
  const client = sql(process.env.DATABASE_URL, {
    connect_timeout: 3, // seconds — per attempt, not per loop
  })
  try {
    await client`select 1`
  } finally {
    await client.end({ timeout: 1 }).catch(() => {})
  }
}

async function redisReady() {
  const Redis = require('ioredis')
  const client = new Redis(process.env.REDIS_URL, {
    lazyConnect: true,
    maxRetriesPerRequest: 0,
    connectTimeout: 3_000,
  })
  try {
    await client.connect()
    await client.ping()
  } finally {
    client.disconnect()
  }
}

let lastError = null
while (Date.now() - started < DEADLINE_MS) {
  try {
    await pgReady()
    await redisReady()
    console.log('[await-deps] postgres + redis are ready')
    process.exit(0)
  } catch (err) {
    lastError = err
    await new Promise((r) => setTimeout(r, RETRY_MS))
  }
}

console.error(
  `[await-deps] dependencies not ready after ${Math.round(DEADLINE_MS / 1000)}s: ` +
    `${lastError instanceof Error ? lastError.message : String(lastError)} — ` +
    `check DATABASE_URL (${process.env.DATABASE_URL ? 'set' : 'UNSET'}) and REDIS_URL ` +
    `(${process.env.REDIS_URL ? 'set' : 'UNSET'}), and that postgres/redis are healthy.`,
)
process.exit(1)
