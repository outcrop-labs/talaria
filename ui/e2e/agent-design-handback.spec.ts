// A design handed back into an empty New agent never takes the place of what
// is being typed there (NN12 of #700's review). A hire that fails before the
// agent exists hands its design back into the person's slot when the slot is
// empty — and an empty slot is exactly what a New agent the person has just
// started typing into looks like to the server: a describe not yet sent, or a
// manual form with no name yet, is saved nowhere. The modal used to take the
// returned design in on its next read, over the typing.
//
// The hire is a stand-in, and everything else is real. The spec writes an
// `agent-hire` runs row (`awaiting`, then `error` with no checkpoint: a
// failure before the agent existed) and moves the design out of the slot onto
// it the way the hire route's claim does (talaria-agent-design store.rs,
// CLAIM). The api settles the failure on the open modal's own poll and hands
// the design back; the modal is the shipped one, in a real browser. No
// container, no model: a design is made through the api's manual start.
import { randomBytes, randomUUID } from 'node:crypto'
import { expect, test, type Browser, type BrowserContext, type Page } from '@playwright/test'
import { BASE_URL } from './support'

type Sql = import('postgres').Sql
type RedisClient = import('ioredis').Redis

let sql: Sql
let redis: RedisClient

test.beforeAll(async () => {
  const dbUrl = process.env.E2E_DATABASE_URL
  const redisUrl = process.env.E2E_REDIS_URL
  if (!dbUrl || !redisUrl) throw new Error('E2E_DATABASE_URL and E2E_REDIS_URL must name the stack’s own Postgres and Redis (docs/E2E.md)')
  const { default: postgres } = await import('postgres')
  const { Redis } = await import('ioredis')
  sql = postgres(dbUrl, { max: 2, onnotice: () => {} })
  redis = new Redis(redisUrl, { maxRetriesPerRequest: 2 })
})

test.afterAll(async () => {
  await sql?.end({ timeout: 2 })
  redis?.disconnect()
})

interface Person {
  id: string
  sid: string
}

const slotKey = (id: string) => `agent-design:${id}`
const hiresKey = (id: string) => `agent-design:${id}:hires`

/** A throwaway admin (a hire needs `agents.manage`) with nothing in New
 *  agent, and a session minted the way a sign-in makes one. */
async function person(key: string): Promise<Person> {
  const sub = `e2e:design:${key}`
  const email = `design-${key}@agent-design.invalid`
  const name = `Design ${key}`
  const [row] = await sql<Array<{ id: string }>>`
    insert into users (sub, email, name, role) values (${sub}, ${email}, ${name}, 'admin')
    on conflict (sub) do update set role = 'admin'
    returning id::text`
  const id = row!.id
  await redis.del(slotKey(id), hiresKey(id), `${slotKey(id)}:watch`)
  const sid = `e2e-design-${randomBytes(20).toString('hex')}`
  await redis.set(`sess:${sid}`, JSON.stringify({ id, sub, email, name, picture: null, role: 'admin', provider: 'password' }), 'EX', 3600)
  return { id, sid }
}

async function contextFor(browser: Browser, p: Person): Promise<BrowserContext> {
  const host = new URL(BASE_URL).hostname
  const ctx = await browser.newContext({
    baseURL: BASE_URL,
    viewport: { width: 1440, height: 960 },
    storageState: {
      cookies: [{ name: 'talaria_session', value: p.sid, domain: host, path: '/', expires: -1, httpOnly: true, secure: false, sameSite: 'Lax' }],
      origins: [],
    },
  })
  // The server never learns of a push subscription from a test browser.
  await ctx.route('**/api/push/subscribe', (r) => r.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' }))
  return ctx
}

interface Stored {
  id: string
  draft: { name: string; role: string; department: string } | null
  releasedFrom?: string | null
}

const slot = async (p: Person): Promise<Stored | null> => {
  const json = await redis.hget(slotKey(p.id), 'json')
  return json ? (JSON.parse(json) as Stored) : null
}
const held = async (p: Person): Promise<Array<{ design: Stored; hireRunId: string; returned: string | null }>> =>
  Object.values(await redis.hgetall(hiresKey(p.id))).map((raw) => JSON.parse(raw))

/** A design on a hire still in flight: saved through the api as a manual
 *  start, then claimed onto a stand-in hire run. */
async function onAHire(page: Page, p: Person, name: string): Promise<{ designId: string; hireId: string }> {
  const r = await page.request.patch('/api/fleet/agent-design', {
    data: { name, handle: name.toLowerCase().replace(/[^a-z0-9]/g, ''), department: `dept-${randomUUID().slice(0, 8)}`, role: 'Scout', soul: 'Watches the deploy trains.', skills: [] },
  })
  expect(r.status(), 'PATCH /api/fleet/agent-design').toBe(200)
  const json = await redis.hget(slotKey(p.id), 'json')
  const rev = await redis.hget(slotKey(p.id), 'rev')
  expect(json && rev, 'the manual start is in the slot').toBeTruthy()
  const design = JSON.parse(json!) as Stored
  const hireId = randomUUID()
  await sql`insert into runs (id, kind, state, input) values (${hireId}::uuid, 'agent-hire', 'awaiting', ${sql.json({ slug: 'nosuchagent', managerUserId: p.id })})`
  // store.rs CLAIM: the slot's record, at the revision read, into the hires.
  const holdJson = JSON.stringify({ design: { ...design, error: null, releasedFrom: null, hireRunId: null }, hireRunId: hireId, claimedAt: Date.now(), returned: null, noticed: false })
  const moved = await redis.eval(
    "if (redis.call('HGET', KEYS[1], 'rev') or '') ~= ARGV[1] then return 0 end redis.call('HSET', KEYS[2], ARGV[2], ARGV[3]) redis.call('EXPIRE', KEYS[2], 3600) redis.call('DEL', KEYS[1]) return 1",
    2,
    slotKey(p.id),
    hiresKey(p.id),
    rev!,
    design.id,
    holdJson,
  )
  expect(moved).toBe(1)
  return { designId: design.id, hireId }
}

/** The hire fails before the agent existed: the design comes back on the
 *  next read that settles it — here, the open modal's own poll. */
async function failBeforeCreate(hireId: string) {
  await sql`update runs set state = 'error', error = 'the department was taken a moment ago' where id = ${hireId}::uuid`
}

async function openNewAgent(page: Page) {
  await page.goto('/agents')
  await page.getByRole('button', { name: 'New agent', exact: true }).click()
  await expect(page.getByRole('dialog')).toBeVisible()
}

async function tidy(p: Person, hireIds: string[]) {
  await redis.del(slotKey(p.id), hiresKey(p.id), `${slotKey(p.id)}:watch`)
  for (const h of hireIds) await sql`delete from runs where id = ${h}::uuid`
  await sql`delete from notifications where user_id = ${p.id}::uuid`
}

test('a design handed back under a describe being typed leaves the typing where it is', async ({ browser }) => {
  const p = await person('describe')
  const ctx = await contextFor(browser, p)
  const page = await ctx.newPage()
  const hires: string[] = []
  try {
    const { designId, hireId } = await onAHire(page, p, 'Handback Scout')
    hires.push(hireId)
    await openNewAgent(page)
    const describe = page.getByPlaceholder(/A release manager/)
    const typed = 'An agent that reads every incident review and drafts the follow-up tickets'
    await describe.fill(typed)

    await failBeforeCreate(hireId)
    // The api handed it back into the empty slot, on the modal's poll.
    await expect.poll(async () => (await slot(p))?.id, { timeout: 30_000 }).toBe(designId)
    await page.waitForTimeout(6_000) // the read that brought it, and one more poll
    // The typing stays, on the describe step, and the modal says the design
    // is back instead of putting it there.
    await expect(describe).toHaveValue(typed, { timeout: 1_000 })
    await expect(page.locator('#cam-name')).toHaveCount(0)
    const arrived = page.getByTestId('design-arrived')
    await expect(arrived).toContainText('The hire of Handback Scout did not finish')
    await expect(arrived).toContainText('the department was taken a moment ago')

    // Opening it is the person's choice: then the review step shows it.
    await arrived.getByRole('button', { name: 'Open it' }).click()
    await expect(page.locator('#cam-name')).toHaveValue('Handback Scout')
    await expect(page.getByTestId('design-hire-error')).toContainText('the department was taken a moment ago')
  } finally {
    await ctx.close()
    await tidy(p, hires)
  }
})

test('a design handed back under a manual form being typed waits for it once it is named', async ({ browser }) => {
  const p = await person('manual')
  const ctx = await contextFor(browser, p)
  const page = await ctx.newPage()
  const hires: string[] = []
  try {
    const { designId, hireId } = await onAHire(page, p, 'Handback Scout')
    hires.push(hireId)
    await openNewAgent(page)
    await page.getByRole('button', { name: /Configure manually/ }).click()
    // No name or handle yet, so nothing of this is saved anywhere.
    await page.locator('#cam-role').fill('Incident reviewer')
    await page.locator('#cam-department').fill('postmortems-e2e')

    await failBeforeCreate(hireId)
    await expect.poll(async () => (await slot(p))?.id, { timeout: 30_000 }).toBe(designId)
    await page.waitForTimeout(6_000)
    await expect(page.locator('#cam-role')).toHaveValue('Incident reviewer', { timeout: 1_000 })
    await expect(page.locator('#cam-department')).toHaveValue('postmortems-e2e')
    await expect(page.locator('#cam-name')).toHaveValue('')
    await expect(page.getByTestId('design-arrived')).toContainText('The hire of Handback Scout did not finish')

    // Naming it saves it: the typed one takes New agent, and the returned
    // design steps aside to wait for it rather than be lost or written over.
    await page.locator('#cam-name').fill('Typed Agent')
    await expect.poll(async () => (await slot(p))?.draft?.name, { timeout: 15_000 }).toBe('Typed Agent')
    const mine = await slot(p)
    expect(mine?.draft?.role).toBe('Incident reviewer')
    expect(mine?.draft?.department).toBe('postmortems-e2e')
    const waiting = await held(p)
    expect(waiting.map((h) => h.design.id)).toEqual([designId])
    expect(waiting[0]!.returned).toContain('the department was taken a moment ago')
    await expect(page.getByTestId('design-returned')).toContainText('The hire of Handback Scout did not finish')
    await expect(page.getByTestId('design-arrived')).toHaveCount(0)

    // Discarding the typed one brings the returned design back, as it was.
    await page.getByTestId('design-discard').click()
    await expect.poll(async () => (await slot(p))?.id, { timeout: 15_000 }).toBe(designId)
    await expect(page.locator('#cam-name')).toHaveValue('Handback Scout', { timeout: 15_000 })
    expect(await held(p)).toEqual([])
  } finally {
    await ctx.close()
    await tidy(p, hires)
  }
})

test('a Create clicked while a manual start’s save is on the wire spends the design it made', async ({ browser }) => {
  // Past the 600 ms debounce the save has gone out. A Create before its
  // answer read no design id and hired without one, leaving a copy of the
  // agent it hired behind in New agent. The save is held on the wire here,
  // and the hire is caught, never sent: what Create asks for is the point.
  const p = await person('inflight')
  const ctx = await contextFor(browser, p)
  const page = await ctx.newPage()
  try {
    await page.route('**/api/fleet/agent-design', async (route) => {
      if (route.request().method() === 'PATCH') await new Promise((r) => setTimeout(r, 2_500))
      await route.continue()
    })
    const caught: { hire: Record<string, unknown> | null } = { hire: null }
    await page.route('**/api/fleet/create', async (route) => {
      caught.hire = route.request().postDataJSON() as Record<string, unknown>
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ok: true, hire: { id: randomUUID(), state: 'queued', phase: 'queued' } }) })
    })
    await openNewAgent(page)
    await page.getByRole('button', { name: /Configure manually/ }).click()
    await page.locator('#cam-department').fill(`inflight-${randomUUID().slice(0, 8)}`)
    await page.locator('#cam-name').fill('Inflight Agent')
    await page.waitForTimeout(1_000) // the save is out, and not yet answered
    await page.getByRole('button', { name: 'Create agent' }).click()
    await expect.poll(() => caught.hire, { timeout: 15_000 }).not.toBeNull()
    const saved = await slot(p)
    expect(saved?.draft?.name, 'the save reached the server before the hire went out').toBe('Inflight Agent')
    expect(caught.hire?.['designId'], 'the hire names the design its own save made').toBe(saved?.id)
  } finally {
    await ctx.close()
    await tidy(p, [])
  }
})
