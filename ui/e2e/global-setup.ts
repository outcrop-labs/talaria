// Before any test: a session, a live api, a live stub, and proof the api can
// reach the stub. Each precondition fails with the sentence that fixes it —
// a suite that dies in test 2 with "unreachable" has explained nothing.
import { randomBytes } from 'node:crypto'
import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname } from 'node:path'
import { request, type FullConfig } from '@playwright/test'
import { BASE_URL, STATE_FILE, STUB, STUB_URL } from './support'

const SESSION_TTL_SECONDS = 24 * 60 * 60

/** A session the way a sign-in makes one: a users row, and `sess:<sid>` in
 *  Redis holding the session user in its stored field order (id, sub,
 *  email, name, picture, role, provider — api/crates/talaria-session). */
async function mintSession(): Promise<string> {
  const dbUrl = process.env.E2E_DATABASE_URL
  const redisUrl = process.env.E2E_REDIS_URL
  if (!dbUrl || !redisUrl) {
    throw new Error(
      'No session: set E2E_SESSION to a signed-in talaria_session cookie value, or E2E_DATABASE_URL and ' +
        "E2E_REDIS_URL to the stack's own Postgres and Redis so the suite can mint an admin session (docs/E2E.md).",
    )
  }
  const { default: postgres } = await import('postgres')
  const { Redis } = await import('ioredis')
  const sql = postgres(dbUrl, { max: 1, onnotice: () => {} })
  const redis = new Redis(redisUrl, { lazyConnect: true, maxRetriesPerRequest: 2 })
  try {
    const [row] = await sql<Array<{ id: string }>>`
      insert into users (sub, email, name, role)
      values ('e2e:admin', 'e2e-admin@talaria.test', 'E2E Admin', 'admin')
      on conflict (sub) do update set role = 'admin', last_seen_at = now()
      returning id::text`
    if (!row) throw new Error('the users upsert returned no row')
    const sid = `e2e-${randomBytes(24).toString('hex')}`
    const user = { id: row.id, sub: 'e2e:admin', email: 'e2e-admin@talaria.test', name: 'E2E Admin', picture: null, role: 'admin', provider: 'password' }
    await redis.connect()
    await redis.set(`sess:${sid}`, JSON.stringify(user), 'EX', SESSION_TTL_SECONDS)
    return sid
  } finally {
    await sql.end({ timeout: 2 })
    redis.disconnect()
  }
}

async function poll<T>(what: string, timeoutMs: number, probe: () => Promise<T | null>): Promise<T> {
  const deadline = Date.now() + timeoutMs
  let last = ''
  for (;;) {
    try {
      const v = await probe()
      if (v !== null) return v
    } catch (e) {
      last = String((e as Error)?.message ?? e)
    }
    if (Date.now() > deadline) throw new Error(`${what} (waited ${Math.round(timeoutMs / 1000)}s)${last ? `: ${last}` : ''}`)
    await new Promise((r) => setTimeout(r, 2_000))
  }
}

export default async function globalSetup(_config: FullConfig) {
  const sid = process.env.E2E_SESSION || (await mintSession())
  const host = new URL(BASE_URL).hostname
  mkdirSync(dirname(STATE_FILE), { recursive: true })
  writeFileSync(
    STATE_FILE,
    JSON.stringify({
      cookies: [{ name: 'talaria_session', value: sid, domain: host, path: '/', expires: -1, httpOnly: true, secure: false, sameSite: 'Lax' }],
      origins: [],
    }),
  )

  const ctx = await request.newContext({ baseURL: BASE_URL, storageState: STATE_FILE, ignoreHTTPSErrors: true })
  try {
    // A worktree stack compiles its api for minutes after the app is up;
    // the app answers 502 until then. Wait it out rather than fail it.
    await poll(
      `the api behind ${BASE_URL} never answered /api/me with this session — is the stack up, and is the session valid?`,
      Number(process.env.E2E_API_WAIT_MS || 600_000),
      async () => ((await ctx.get('/api/me')).status() === 200 ? true : null),
    )
    await poll(
      `the stub at ${STUB_URL} is not answering — start it: node scripts/e2e/mcp-stubs.mjs (E2E_STUB_PORT sets its port)`,
      30_000,
      async () => ((await ctx.get(`${STUB_URL}/_e2e/health`)).ok() ? true : null),
    )
    // The api reaches the stub through its SSRF guard and its own TLS roots;
    // both have to be told about a loopback stub with a private CA.
    const res = await ctx.post('/api/mcp/test', { data: { url: STUB.open.url } })
    const probe = res.ok() ? ((await res.json()) as { state: string; detail?: string }) : null
    if (probe?.state !== 'ok') {
      const why = new Error(
        `the api cannot reach the stub at ${STUB.open.url} (${probe ? `${probe.state}: ${probe.detail ?? ''}` : `HTTP ${res.status()}`}). ` +
          'Start the api with the env the stub prints (node scripts/e2e/mcp-stubs.mjs --init): ' +
          'TALARIA_FETCH_ALLOW_HOSTS=127.0.0.1, SSL_CERT_FILE=<the stub CA bundle>, TALARIA_MCP_REGISTRY_URL=<stub>/v0.1/servers.',
      )
      // E2E_PREFLIGHT=warn runs the tests anyway, so each one shows how far
      // it gets — for diagnosing a stack, never for a gate.
      if (process.env.E2E_PREFLIGHT !== 'warn') throw why
      console.warn(`[e2e] preflight: ${why.message}`)
    }
  } finally {
    await ctx.dispose()
  }
}
