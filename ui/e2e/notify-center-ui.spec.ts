// The notification center end to end (docs/notifications/center.md): the
// bell, the /notifications page, the category × channel matrix, pause and
// working hours, and the status panel — in a real browser against a real
// api, the way a person uses them.
//
// Each chapter makes its own throwaway people (e2e:nc:* on .invalid
// addresses, every class routed in-app so nothing is ever mailed) and their
// sessions in the stack's own Redis, seeds rows straight into Postgres, and
// clears what it seeded first, so a rerun starts clean. NOTHING HERE REACHES A
// PUSH SERVICE: every context refuses /api/push/subscribe (the browser may
// still subscribe with its own push service; the server never learns of it),
// and the "devices" are rows pointing at a loopback port.
//
// Chapters that need the inbox API (Done, mark unread, paging, the server's
// filters) skip on an api whose `features` lacks `inbox-v2`, and say so.
import { createHash, randomBytes, randomUUID } from 'node:crypto'
import { chromium, expect, test, type Browser, type BrowserContext, type Page } from '@playwright/test'
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

// ── People ──────────────────────────────────────────────────────────────────

const IN_APP = {
  mention: 'in_app',
  dm: 'in_app',
  approval_pending: 'in_app',
  judge_escalation: 'in_app',
  agent_blocked: 'in_app',
  gap_reported: 'in_app',
  work_complete: 'in_app',
  workchain_turn: 'in_app',
  workchain_paused: 'in_app',
  digest: 'off',
}

interface Person {
  id: string
  sub: string
  email: string
  name: string
  role: 'admin' | 'member'
  sid: string
}

/** A fresh throwaway person: their rows, devices, test budget and settings
 *  cleared, and a session minted the way a sign-in makes one. */
async function person(key: string, role: 'admin' | 'member' = 'member', timezone: string | null = null): Promise<Person> {
  const sub = `e2e:nc:${key}`
  const email = `nc-${key}@notify-center.invalid`
  const name = `NC ${key[0]!.toUpperCase()}${key.slice(1)}`
  const [row] = await sql<Array<{ id: string }>>`
    insert into users (sub, email, name, role, notify_prefs, timezone)
    values (${sub}, ${email}, ${name}, ${role}, ${sql.json(IN_APP)}, ${timezone})
    on conflict (sub) do update set role = excluded.role, notify_prefs = excluded.notify_prefs, timezone = excluded.timezone
    returning id::text`
  const id = row!.id
  await sql`delete from notifications where user_id = ${id}::uuid`
  await sql`delete from push_subscriptions where user_id = ${id}::uuid`
  await redis.del(`talaria:rl:notify:test:${id}`)
  const sid = `e2e-nc-${randomBytes(20).toString('hex')}`
  await redis.set(`sess:${sid}`, JSON.stringify({ id, sub, email, name, picture: null, role, provider: 'password' }), 'EX', 3600)
  return { id, sub, email, name, role, sid }
}

async function contextFor(browser: Browser, p: Person, opts: { timezoneId?: string; init?: () => void } = {}): Promise<BrowserContext> {
  const host = new URL(BASE_URL).hostname
  const ctx = await browser.newContext({
    baseURL: BASE_URL,
    viewport: { width: 1440, height: 960 },
    timezoneId: opts.timezoneId,
    storageState: {
      cookies: [{ name: 'talaria_session', value: p.sid, domain: host, path: '/', expires: -1, httpOnly: true, secure: false, sameSite: 'Lax' }],
      origins: [],
    },
  })
  // The server never learns of a push subscription from a test browser.
  await ctx.route('**/api/push/subscribe', (r) => r.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' }))
  if (opts.init) await ctx.addInitScript(opts.init)
  return ctx
}

interface Payload {
  unread: number
  unreadForYou?: number
  prefs: Record<string, string>
  channels?: Record<string, { inApp: true; desktop: boolean; push: boolean; email: boolean }>
  classes?: Array<{ id: string; category: string; adminOnly: boolean; kinds: Array<{ id: string }> }>
  dnd?: { pausedUntil: string | null; active: boolean; reason: string | null; until: string | null; timeZone: string; schedule: unknown }
  emailFrequency?: string
  features?: string[]
  delivery: { emailEnabled: boolean }
  notifications: Array<{ id: string; readAt: string | null; archivedAt?: string | null }>
}

const getPayload = async (page: Page): Promise<Payload> => {
  const r = await page.request.get('/api/notifications')
  expect(r.status(), 'GET /api/notifications').toBe(200)
  return (await r.json()) as Payload
}

// ── Rows ────────────────────────────────────────────────────────────────────

interface Seed {
  kind: string
  title: string
  body?: string
  href?: string
  count?: number
  entity?: [string, string]
  actor?: [string, string | null, string | null]
  readAt?: string | null
}

/** Rows straight into the table, newest first in the order given. */
async function seed(p: Person, rows: Seed[], start = Date.now()): Promise<string[]> {
  const ids: string[] = []
  for (const [i, r] of rows.entries()) {
    const [row] = await sql<Array<{ id: string }>>`
      insert into notifications (user_id, kind, title, body, href, count, created_at, read_at,
                                 entity_type, entity_id, actor_kind, actor_id, actor_label, alert)
      values (${p.id}::uuid, ${r.kind}, ${r.title}, ${r.body ?? ''}, ${r.href ?? ''}, ${r.count ?? 1},
              ${new Date(start - i * 60_000).toISOString()}::timestamptz, ${r.readAt ?? null},
              ${r.entity?.[0] ?? null}, ${r.entity?.[1] ?? null},
              ${r.actor?.[0] ?? null}, ${r.actor?.[1] ?? null}, ${r.actor?.[2] ?? null}, 'banner')
      returning id::text`
    ids.push(row!.id)
  }
  return ids
}

/** 2 Needs you, 3 Direct (one a DM coalesced three times) and 4 Following
 *  (three about one ticket) — all unread. */
function bellRows(other: Person): Seed[] {
  const task = randomUUID()
  return [
    { kind: 'approval_pending', title: 'Approve the api deploy', href: '/home/inbox' },
    { kind: 'approval_pending', title: 'Approve the Q4 budget', href: '/home/inbox' },
    { kind: 'mention', title: 'NC Other mentioned you', body: 'in #general', href: '/settings/profile', actor: ['user', other.id, null] },
    { kind: 'task-assigned', title: 'Assigned: TAL-7 Fix the login', href: '/home/inbox' },
    { kind: 'dm', title: 'NC Other sent you a message', body: 'the third one', count: 3, href: '/home/inbox', entity: ['channel', randomUUID()], actor: ['user', other.id, null] },
    { kind: 'task-status', title: 'TAL-112 moved to Review', href: '/home/inbox', entity: ['task', task] },
    { kind: 'task-status', title: 'TAL-112 moved to In progress', href: '/home/inbox', entity: ['task', task] },
    { kind: 'task-status', title: 'TAL-112 moved to Todo', href: '/home/inbox', entity: ['task', task] },
    { kind: 'research', title: 'Research ready: market sizing', href: '/home/inbox', actor: ['agent', 'research', 'Research agent'] },
  ]
}

const bell = (page: Page) => page.getByRole('button', { name: 'Notifications', exact: true })
/** Settings → Notifications keeps what only explains a miss (this browser's
 *  checks, the live connection) behind a closed Troubleshooting card. The
 *  card stays mounted, so its readings are live before it is opened. */
const openTroubleshooting = (page: Page) => page.getByRole('button', { name: 'Troubleshooting' }).click()
const bellRowsIn = (page: Page) => page.locator('[data-bell-list] [data-notification-row]')

async function openBell(page: Page) {
  await bell(page).click()
  await expect(page.locator('[data-bell-list], [data-bell-view-all]').first()).toBeVisible()
}

// ── 1. The bell ─────────────────────────────────────────────────────────────

test.describe('the bell', () => {
  test('badges For you, slices For you and All, groups and counts, opens in place', async ({ browser }) => {
    const other = await person('other')
    const me = await person('bell')
    const ids = await seed(me, bellRows(other))
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.goto('/home')
    const p0 = await getPayload(page)
    expect(p0.unread).toBe(9)
    expect(p0.unreadForYou).toBe(5)

    // Badge = unreadForYou (O-10).
    await expect(page.locator('[data-notify-badge]')).toHaveAttribute('data-notify-badge', '5')

    await openBell(page)
    await page.getByRole('button', { name: 'For you' }).click()
    await expect(bellRowsIn(page)).toHaveCount(5)
    await page.getByRole('button', { name: 'All', exact: true }).click()
    await expect(bellRowsIn(page)).toHaveCount(9)

    // A coalesced row says its count; rows about one ticket share a header.
    await expect(page.locator(`[data-notification-row="${ids[4]}"] [data-row-meta]`)).toContainText('3 messages')
    const group = page.locator('[data-bell-list] [data-notification-group^="task:"]')
    await expect(group).toHaveCount(1)
    await expect(group).toContainText('TAL-112 · 3 updates')
    await expect(group.locator('[data-notification-row]')).toHaveCount(3)

    // Every ungrouped row's text starts at one x: a row with no actor shows
    // its category's glyph where the avatar would be.
    const lefts = await page
      .locator('[data-bell-list] > [data-notification-row] [data-row-meta]')
      .evaluateAll((els) => els.map((e) => Math.round(e.getBoundingClientRect().left)))
    expect(lefts.length).toBeGreaterThan(3)
    expect(new Set(lefts).size).toBe(1)
    await expect(page.locator('[data-bell-list] [data-row-glyph="needs_you"]').first()).toBeVisible()

    // Without the inbox API there is no Done, and nothing else breaks.
    const v2 = (p0.features ?? []).includes('inbox-v2')
    if (!v2) {
      await expect(page.locator('[data-bell-list] button[title="Done"]')).toHaveCount(0)
    }

    // Open: in place (no document load), marked read.
    await page.evaluate(() => ((window as unknown as { __nc: number }).__nc = 1))
    await page.locator(`[data-notification-row="${ids[2]}"] button[title="NC Other mentioned you"]`).click()
    await expect(page).toHaveURL(/\/settings\/profile$/)
    expect(await page.evaluate(() => (window as unknown as { __nc?: number }).__nc)).toBe(1)
    await expect
      .poll(async () => (await sql`select read_at from notifications where id = ${ids[2]!}::uuid`)[0]?.read_at ?? null)
      .not.toBeNull()
    await expect(page.locator('[data-notify-badge]')).toHaveAttribute('data-notify-badge', '4')

    // The full page renders the same rows, with no Done view on an api
    // without the inbox API.
    await page.goto('/notifications?view=all')
    await expect(page.locator('[data-center-list] [data-notification-row]')).toHaveCount(9)
    if (!v2) await expect(page.getByRole('button', { name: 'Done', exact: true })).toHaveCount(0)
    await ctx.close()
  })
})

test.describe('narrow windows', () => {
  test('the bell stays on screen and each kind keeps its label and its control readable at 390 px', async ({ browser }) => {
    const other = await person('other')
    const me = await person('narrow')
    await seed(me, bellRows(other))
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.setViewportSize({ width: 390, height: 844 })
    await page.goto('/home')
    await openBell(page)
    const panel = await page.locator('[data-bell-panel]').boundingBox()
    expect(panel, 'the bell panel').not.toBeNull()
    expect(panel!.x).toBeGreaterThanOrEqual(0)
    expect(panel!.x + panel!.width).toBeLessThanOrEqual(390)
    await page.keyboard.press('Escape')

    await page.goto('/settings/notifications')
    const label = page.locator('[data-matrix-row="dm"] [data-settings-row] > div').first()
    await expect(label).toBeVisible()
    // The label spans the row; the control sits under it, on screen, and names the channels in words.
    expect((await label.boundingBox())!.width).toBeGreaterThan(240)
    const control = page.locator('[data-matrix-row="dm"] [data-matrix-menu]')
    await expect(control).toContainText(/push/i)
    const box = (await control.boundingBox())!
    expect(box.x).toBeGreaterThanOrEqual(0)
    expect(box.x + box.width).toBeLessThanOrEqual(390)
    await ctx.close()
  })
})

// ── 2. The matrix ───────────────────────────────────────────────────────────

test.describe('the matrix', () => {
  test('its rows are the served classes, for a member and for an admin', async ({ browser }) => {
    // The registry grows (W2-E adds `account`), so the counts are read from
    // the server: the contract alone serves 9 and 11.
    const served: Record<string, number> = {}
    let adminOnly = 0
    for (const role of ['member', 'admin'] as const) {
      const p = await person(`matrix-${role}`, role)
      const ctx = await contextFor(browser, p)
      const page = await ctx.newPage()
      await page.goto('/settings/notifications')
      const payload = await getPayload(page)
      const classes = payload.classes ?? []
      served[role] = classes.length
      if (role === 'member') expect(classes.filter((c) => c.adminOnly), 'a member is served no admin-only class').toEqual([])
      else adminOnly = classes.filter((c) => c.adminOnly).length
      await expect(page.locator('[data-matrix-row]')).toHaveCount(classes.length)
      const shown = await page.locator('[data-matrix-row]').evaluateAll((els) => els.map((e) => e.getAttribute('data-matrix-row')))
      const known = ['needs_you', 'direct', 'following', 'workspace']
      expect(shown, 'the rows, in registry order within their groups').toEqual([
        ...known.flatMap((cat) => classes.filter((c) => c.category === cat).map((c) => c.id)),
        ...classes.filter((c) => !known.includes(c.category)).map((c) => c.id),
      ])
      // In-app cannot be turned off: the page says the inbox keeps everything,
      // and a kind's menu offers only the three channels beyond it.
      await expect(page.getByText('Everything is kept in your inbox')).toBeVisible()
      await page.locator('[data-matrix-row="dm"] [data-matrix-menu]').click()
      await expect(page.getByRole('menuitem')).toHaveText([/Desktop/, /Push/, /Email/])
      await page.keyboard.press('Escape')
      await expect(page.getByRole('menu')).toHaveCount(0)
      await ctx.close()
    }
    expect(adminOnly).toBeGreaterThan(0)
    expect(served.admin).toBe(served.member! + adminOnly)
    test.info().annotations.push({ type: 'classes', description: `member ${served.member}, admin ${served.admin}` })
  })

  test('Push off for direct messages saves, survives a reload and derives the legacy route', async ({ browser }) => {
    const p = await person('matrix-push')
    const ctx = await contextFor(browser, p)
    const page = await ctx.newPage()
    await page.goto('/settings/notifications')
    // The row carries its saved channels; the menu button says them in words.
    const dm = page.locator('[data-matrix-row="dm"]')
    await expect(dm).toHaveAttribute('data-push', 'true')
    await expect(dm.locator('[data-matrix-menu]')).toContainText(/push/i)
    const saved = page.waitForResponse((r) => r.url().endsWith('/api/notifications') && r.request().method() === 'PATCH')
    await dm.locator('[data-matrix-menu]').click()
    await page.getByRole('menuitem', { name: 'Push' }).click()
    const res = await saved
    expect(res.status()).toBe(200)
    expect(JSON.parse(res.request().postData() ?? '{}')).toEqual({ channels: { dm: { push: false } } })
    await page.reload()
    await expect(dm).toHaveAttribute('data-push', 'false')
    await expect(dm.locator('[data-matrix-menu]')).not.toContainText(/push/i)
    const payload = await getPayload(page)
    const dm = payload.channels!.dm!
    expect(dm.push).toBe(false)
    // The legacy route derives from the channels (I-13): email and a screen →
    // both, email alone → email, no email → in_app.
    const legacy = !dm.email ? 'in_app' : dm.desktop || dm.push ? 'both' : 'email'
    expect(payload.prefs.dm).toBe(legacy)
    const [stored] = await sql<Array<{ push: string | null }>>`select notify_prefs->'channels'->'dm'->>'push' as push from users where id = ${p.id}::uuid`
    expect(stored?.push).toBe('false')
    // Reset to default, in the same menu, puts it back.
    await dm.locator('[data-matrix-menu]').click()
    await page.getByRole('menuitem', { name: 'Reset to default' }).click()
    await expect(dm).toHaveAttribute('data-push', 'true')
    await expect(dm).toHaveAttribute('data-at-default', 'true')
    await ctx.close()
  })

  test('instance email off disables every Email choice with the explanation; the frequency round-trips', async ({ browser }) => {
    const p = await person('matrix-email')
    const ctx = await contextFor(browser, p)
    const page = await ctx.newPage()
    await page.goto('/settings/notifications')
    const payload = await getPayload(page)
    if (payload.delivery.emailEnabled) test.skip(true, 'instance email is on in this stack; the off state is the one under test')
    await expect(page.locator('[data-matrix-email-off]')).toContainText('Email is off for this workspace')
    const rows = page.locator('[data-matrix-row]')
    expect(await rows.count()).toBe(payload.classes!.length)
    for (const row of await rows.all()) {
      await row.locator('[data-matrix-menu]').click()
      const email = page.getByRole('menuitem', { name: 'Email' })
      await expect(email).toBeDisabled()
      await expect(email).toContainText('Off for this workspace')
      await expect(page.getByRole('menuitem', { name: 'Push' })).toBeEnabled()
      await page.keyboard.press('Escape')
      await expect(page.getByRole('menu')).toHaveCount(0)
    }

    expect(payload.emailFrequency).toBe('1h')
    const saved = page.waitForResponse((r) => r.url().endsWith('/api/notifications') && r.request().method() === 'PATCH')
    await page.locator('[data-email-frequency]').getByRole('button', { name: 'Daily' }).click()
    expect(JSON.parse((await saved).request().postData() ?? '{}')).toEqual({ emailFrequency: '1d' })
    await page.reload()
    await expect(page.locator('[data-email-frequency]')).toHaveAttribute('data-email-frequency', '1d')
    expect((await getPayload(page)).emailFrequency).toBe('1d')
    await ctx.close()
  })
})

// ── 3. Pause and working hours ──────────────────────────────────────────────

/** 09:00 on the next calendar day in `zone`, computed independently of the
 *  app: the one candidate offset whose wall clock reads back as 09:00. */
function tomorrowNine(now: number, zone: string): number {
  const f = new Intl.DateTimeFormat('en-CA', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })
  const parts = (t: number) => Object.fromEntries(f.formatToParts(t).map((x) => [x.type, x.value]))
  const today = parts(now)
  const d = new Date(Date.UTC(Number(today.year), Number(today.month) - 1, Number(today.day) + 1, 9))
  for (let h = -14; h <= 14; h++) {
    const t = d.getTime() - h * 3_600_000
    const w = parts(t)
    if (Number(w.day) === d.getUTCDate() && w.hour === '09' && w.minute === '00') return t
  }
  throw new Error(`no 09:00 tomorrow in ${zone}`)
}

test.describe('pause and working hours', () => {
  test('account menu → pause 1 h shows in the bell and stores ≈ +1 h; Resume clears it', async ({ browser }) => {
    const p = await person('pause')
    const ctx = await contextFor(browser, p)
    const page = await ctx.newPage()
    await page.goto('/home')
    await page.getByRole('button', { name: p.name }).click()
    await page.locator('[data-user-menu-pause]').click()
    const saved = page.waitForResponse((r) => r.url().endsWith('/api/notifications') && r.request().method() === 'PATCH')
    const t = Date.now()
    await page.locator('[data-pause-preset="1h"]').click()
    expect((await saved).status()).toBe(200)
    // The menu closes itself once the pause is saved.
    await expect(page.locator('[data-user-menu-pause]')).toHaveCount(0)
    const dnd = (await getPayload(page)).dnd!
    expect(Math.abs(Date.parse(dnd.pausedUntil!) - (t + 3_600_000))).toBeLessThan(120_000)
    expect(dnd.active).toBe(true)
    await openBell(page)
    await expect(page.locator('[data-dnd-banner="paused"] [data-dnd-label]')).toContainText('Paused until')
    await page.locator('[data-dnd-resume]').click()
    await expect(page.locator('[data-dnd-banner]')).toHaveCount(0)
    expect((await getPayload(page)).dnd!.pausedUntil).toBeNull()
    await ctx.close()
  })

  test('"until tomorrow 9:00" is 9:00 in the person’s zone (Pacific/Auckland), not the browser’s', async ({ browser }) => {
    const p = await person('auckland', 'member', 'Pacific/Auckland')
    const ctx = await contextFor(browser, p, { timezoneId: 'Europe/London' })
    const page = await ctx.newPage()
    await page.goto('/home')
    await openBell(page)
    await page.getByRole('button', { name: 'Pause notifications' }).click()
    await expect(page.locator('[data-pause-preset="tomorrow"]')).toContainText('Pacific/Auckland')
    const t = Date.now()
    const saved = page.waitForResponse((r) => r.url().endsWith('/api/notifications') && r.request().method() === 'PATCH')
    await page.locator('[data-pause-preset="tomorrow"]').click()
    await saved
    const dnd = (await getPayload(page)).dnd!
    expect(dnd.timeZone).toBe('Pacific/Auckland')
    expect(Date.parse(dnd.pausedUntil!)).toBe(tomorrowNine(t, 'Pacific/Auckland'))
    await ctx.close()
  })

  test('the schedule saves, and outside its hours GET reports DND active', async ({ browser }) => {
    const p = await person('schedule')
    const ctx = await contextFor(browser, p)
    const page = await ctx.newPage()
    await page.goto('/settings/notifications')
    const box = page.locator('[data-notify-schedule]')
    await box.getByRole('switch').first().click()
    await expect(box.locator('[data-schedule-days]')).toBeVisible()
    // The save is confirmed in the page header, where the page says it autosaves.
    await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible()
    // Every day, and hours that exclude now (the person has no zone: UTC).
    for (const d of ['Sat', 'Sun']) await box.locator('[data-schedule-days]').getByRole('button', { name: d }).click()
    const hh = (n: number) => String(((new Date().getUTCHours() + n) % 24 + 24) % 24).padStart(2, '0')
    await box.locator('[data-schedule-start]').fill(`${hh(2)}:00`)
    await box.locator('[data-schedule-end]').fill(`${hh(3)}:00`)
    await expect
      .poll(async () => {
        const d = (await getPayload(page)).dnd!
        return `${d.active}/${d.reason}/${JSON.stringify(d.schedule)}`
      })
      .toBe(`true/schedule/${JSON.stringify({ enabled: true, days: [0, 1, 2, 3, 4, 5, 6], start: `${hh(2)}:00`, end: `${hh(3)}:00` })}`)
    await page.reload()
    await expect(page.locator('[data-schedule-now]')).toContainText('outside working hours')
    await ctx.close()
  })
})

// ── 4. The status panel ─────────────────────────────────────────────────────

const sessionHash = (sid: string) => createHash('sha256').update(sid).digest('hex')

async function device(p: Person, ua: string, endpoint: string, sid: string | null) {
  await sql`
    insert into push_subscriptions (user_id, endpoint, p256dh, auth, session_hash, user_agent)
    values (${p.id}::uuid, ${endpoint}, ${'BNcRdreALRFXTkOOUHK1EtK2wtaz5Ry4YfYCA_0QTpQtUbVlUls0VJXg7A8u-Ts1XbjhazAkj7I99e8QcYP7DkM'},
            ${'tBHItJI5svbpez7KI4CCXg'}, ${sid ? sessionHash(sid) : null}, ${ua})`
}

test.describe('the status panel', () => {
  test('the permission flips live, with no reload', async ({ browser, browserName }) => {
    const p = await person('perm')
    // Chromium's headless shell pins Notification.permission at "denied"
    // whatever is granted; the full browser in headless mode tracks grants.
    let own: Browser | null = null
    if (browserName === 'chromium') {
      own = await chromium.launch({ channel: 'chromium' }).catch(() => null)
      test.skip(!own, 'the full Chromium build (channel "chromium") is not installed here')
    }
    const ctx = await contextFor(own ?? browser, p)
    const page = await ctx.newPage()
    await page.goto('/settings/notifications')
    const perm = page.locator('[data-browser-checks] [data-check="permission"]')
    // Read once while the card is still closed, then opened so the flips
    // below are the ones a person watching the row would see.
    await expect(perm).toHaveAttribute('data-state', 'default')
    await openTroubleshooting(page)
    await expect(perm).toBeVisible()
    await page.evaluate(() => ((window as unknown as { __nc: number }).__nc = 1))
    const origin = new URL(BASE_URL).origin
    if (browserName === 'chromium') {
      // CDP, scoped to this context (the default context is another one).
      const cdp = await ctx.newCDPSession(page)
      const { targetInfo } = await cdp.send('Target.getTargetInfo')
      const set = (setting: 'granted' | 'denied' | 'prompt') =>
        cdp.send('Browser.setPermission', { permission: { name: 'notifications' }, setting, origin, browserContextId: targetInfo.browserContextId })
      await set('granted')
      await expect(perm).toHaveAttribute('data-state', 'granted')
      await expect(page.locator('[data-browser-test]')).toBeVisible()
      await set('denied')
      await expect(perm).toHaveAttribute('data-state', 'denied')
      await expect(page.locator('[data-notify-steps]')).toBeVisible()
      await set('prompt')
      await expect(perm).toHaveAttribute('data-state', 'default')
    } else {
      // Firefox: Playwright grants and clears (it cannot deny).
      await ctx.grantPermissions(['notifications'], { origin })
      await expect(perm).toHaveAttribute('data-state', 'granted')
      await ctx.clearPermissions()
      await expect(perm).toHaveAttribute('data-state', 'default')
    }
    expect(await page.evaluate(() => (window as unknown as { __nc?: number }).__nc)).toBe(1)
    await ctx.close()
    await own?.close()
  })

  test('devices: this browser marked, a test answered, the 6th limited, Remove deletes', async ({ browser }) => {
    const p = await person('devices')
    const capture = process.env.E2E_PUSH_CAPTURE_URL || 'http://127.0.0.1:9/push'
    await device(p, 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15', `${capture}/nc-other-${p.id}`, null)
    await device(p, 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36', `${capture}/nc-this-${p.id}`, p.sid)
    const [mine] = await sql<Array<{ id: string }>>`select id::text from push_subscriptions where user_id = ${p.id}::uuid and session_hash is not null`
    const ctx = await contextFor(browser, p)
    const page = await ctx.newPage()
    await page.goto('/settings/notifications')
    const rows = page.locator('[data-notify-devices] [data-device]')
    await expect(rows).toHaveCount(2)
    await expect(page.locator('[data-this-browser="true"]')).toHaveAttribute('data-device', mine!.id)
    await expect(page.locator('[data-this-browser="true"]')).toContainText('This browser')
    await openTroubleshooting(page)
    await expect(page.locator('[data-check="server"]')).toBeVisible()
    await expect(page.locator('[data-check="server"]')).toHaveAttribute('data-state', 'yes')

    // Send test and Remove browser are in each row's menu.
    const thisRow = page.locator(`[data-device="${mine!.id}"]`)
    const sendTest = async () => {
      await thisRow.getByRole('button', { name: /^Actions for / }).click()
      await page.getByRole('menuitem', { name: 'Send test' }).click()
    }
    const answered = page.waitForResponse((r) => r.url().endsWith('/api/notifications/test'))
    await sendTest()
    const res = await answered
    const result = thisRow.locator('[data-device-result]')
    if (res.status() === 501) {
      // The contract alone: the test send is a stub.
      await expect(result).toHaveText('Server test not available on this instance yet.')
    } else if (res.status() === 200) {
      // With the delivery engine: the push service's answer, either way.
      await expect(result).toHaveAttribute('data-device-result', 'sent')
      await expect(result).toContainText(/Delivered to the push service in \d+ ms|Not delivered/)
    } else {
      // Any other refusal is shown as the server's sentence, not swallowed.
      await expect(result).toHaveAttribute('data-device-result', /^(error|gone)$/)
    }
    for (let i = 2; i <= 6; i++) {
      const r = page.waitForResponse((x) => x.url().endsWith('/api/notifications/test'))
      await sendTest()
      const got = await r
      if (i < 6) expect(got.status()).not.toBe(429)
      else expect(got.status()).toBe(429)
      await expect(thisRow).not.toHaveAttribute('data-device-busy', 'true')
    }
    await expect(result).toHaveAttribute('data-device-result', 'limited')
    await expect(result).toContainText(/Try again in \d+ (min|s)/)

    await page.locator('[data-notify-devices] [data-device]:not([data-this-browser])').getByRole('button', { name: /^Actions for / }).click()
    await page.getByRole('menuitem', { name: 'Remove browser', exact: true }).click()
    await page.locator('[role="dialog"]').getByRole('button', { name: 'Remove' }).click()
    await expect(rows).toHaveCount(1)
    expect((await sql`select count(*)::int as n from push_subscriptions where user_id = ${p.id}::uuid`)[0]?.n).toBe(1)
    // Leave no device behind.
    await sql`delete from push_subscriptions where user_id = ${p.id}::uuid`
    await ctx.close()
  })

  test('alerts go to the active device, by the presence winner', async ({ browser }) => {
    const p = await person('presence')
    const ctx = await contextFor(browser, p)
    const page = await ctx.newPage()
    await page.goto('/settings/notifications')
    const r = await page.request.put('/api/me/presence', { data: { clientId: 'desk-nc-e2e-0001', kind: 'desktop', state: 'active', osIdleMs: 1000 } })
    expect(r.status()).toBe(200)
    await openTroubleshooting(page)
    await expect(page.locator('[data-status-section="live"]')).toBeVisible()
    await expect(page.locator('[data-live="clients"]')).toContainText('Talaria Desktop (active)', { timeout: 20_000 })
    // The winner rule (I-17): the client being looked at, else the active
    // one. A web runtime that heartbeats (W2-R) makes this tab the one being
    // looked at; without it the desktop client is the only one.
    const snap = (await (await page.request.get('/api/me/presence')).json()) as { looking: string | null; active: string | null }
    const winner = snap.looking ?? snap.active
    await expect(page.locator('[data-live="target"]')).toHaveAttribute(
      'data-state',
      winner === 'desk-nc-e2e-0001' ? 'desktop' : /^(this-browser|other-browser)$/,
      { timeout: 20_000 },
    )
    if (winner === 'desk-nc-e2e-0001') await expect(page.locator('[data-live="target"]')).toContainText('Talaria Desktop')
    // How push is routed is said only as this instance does it.
    const gated = ((await getPayload(page)).features ?? []).includes('presence-gated-push')
    await expect(page.locator('[data-live-explain]')).toHaveAttribute('data-live-explain', gated ? 'gated' : 'immediate')
    await ctx.close()
  })

  test('in a faked desktop shell the Talaria Desktop section reads the bridge', async ({ browser }) => {
    const p = await person('shell')
    const ctx = await contextFor(browser, p, {
      init: () => {
        const calls: string[] = []
        const status = {
          backend: 'fdo',
          permission: 'granted',
          server: { name: 'GNOME Shell', vendor: 'GNOME', version: '47.1', spec: '1.2' },
          pending: false,
          closeToBackground: true,
          background: 'tray',
        }
        const w = window as unknown as Record<string, unknown>
        w.__ncCalls = calls
        w.__TAURI_INTERNALS__ = {
          invoke: async (cmd: string) => {
            calls.push(cmd)
            if (cmd === 'desktop_notification_status') return status
            if (cmd === 'desktop_notification_test') return { ok: true, status }
            if (cmd === 'list_instances') return []
            if (cmd === 'get_desktop_settings') return { titlebar: 'themed' }
            return null
          },
          transformCallback: () => 0,
          unregisterCallback: () => {},
          metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        }
      },
    })
    const page = await ctx.newPage()
    await page.goto('/settings/notifications')
    const desk = page.locator('[data-status-section="desktop"]')
    await expect(desk.locator('[data-shell-status="granted"]')).toContainText('On, shown by GNOME Shell 47.1')
    await desk.locator('[data-shell-test]').click()
    await expect(desk.locator('[data-shell-test-result]')).toContainText('Sent.')
    const calls = await page.evaluate(() => (window as unknown as { __ncCalls: string[] }).__ncCalls)
    expect(calls).toContain('desktop_notification_status')
    expect(calls).toContain('desktop_notification_test')
    await ctx.close()
  })
})

// ── 5. Done, filters, paging and the keyboard (the inbox API) ────────────────

async function needsInbox(page: Page) {
  const f = (await getPayload(page)).features ?? []
  test.skip(!f.includes('inbox-v2'), 'this api serves no inbox-v2 (W2-H): no Done, no paging, no server filters')
}

test.describe('the inbox', () => {
  test('Done leaves the views and the badge and lands in Done; Move to inbox restores it read; mark unread counts again', async ({ browser }) => {
    const other = await person('other')
    const me = await person('done')
    const ids = await seed(me, bellRows(other))
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.goto('/home')
    await needsInbox(page)
    await expect(page.locator('[data-notify-badge]')).toHaveAttribute('data-notify-badge', '5')
    await openBell(page)
    await page.getByRole('button', { name: 'For you' }).click()
    for (const id of [ids[0]!, ids[1]!]) {
      await page.locator(`[data-notification-row="${id}"]`).hover()
      await page.locator(`[data-notification-row="${id}"] button[title="Done"]`).click()
      await expect(page.locator(`[data-notification-row="${id}"]`)).toHaveCount(0)
    }
    await expect(page.locator('[data-notify-badge]')).toHaveAttribute('data-notify-badge', '3')
    // The bell drops a row before its write commits: poll the table, don't read it once.
    const archivedIds = async () =>
      (await sql<Array<{ id: string }>>`select id::text from notifications where user_id = ${me.id}::uuid and archived_at is not null and read_at is not null order by id`)
        .map((r) => r.id)
        .sort()
    await expect.poll(archivedIds).toEqual([ids[0]!, ids[1]!].sort())

    await page.goto('/notifications?view=done')
    await expect(page.locator('[data-center-list] [data-notification-row]')).toHaveCount(2)
    await page.locator(`[data-notification-row="${ids[0]}"]`).hover()
    await page.locator(`[data-notification-row="${ids[0]}"] button[title="Move to inbox"]`).click()
    await expect(page.locator('[data-center-list] [data-notification-row]')).toHaveCount(1)
    // The list moves the row out before the PUT commits (seen on Firefox under load): poll.
    const backState = async () => {
      const [r] = await sql<Array<{ archived: boolean; read: boolean }>>`select archived_at is not null as archived, read_at is not null as read from notifications where id = ${ids[0]!}::uuid`
      return `${r?.archived}/${r?.read}`
    }
    await expect.poll(backState).toBe('false/true')

    // Mark unread → +1. A full load leaves the bell at 0 until its first read
    // answers, and `goto` returns before that: wait for the count Done and Move
    // to inbox left (5 − 2, the restored row read), never a one-shot read — on
    // CI's built ui that read landed first and said 0 (expected 1, got 4).
    await page.goto('/notifications')
    await expect(page.locator('[data-notify-badge]')).toHaveAttribute('data-notify-badge', '3')
    await page.locator(`[data-notification-row="${ids[0]}"]`).hover()
    await page.locator(`[data-notification-row="${ids[0]}"] button[title="Mark unread"]`).click()
    await expect(page.locator('[data-notify-badge]')).toHaveAttribute('data-notify-badge', '4')
    await ctx.close()
  })

  test('filters match the server, by an SQL oracle', async ({ browser }) => {
    const other = await person('other')
    const me = await person('filters')
    const ids = await seed(me, bellRows(other))
    await sql`update notifications set read_at = now() where id = ${ids[3]!}::uuid`
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.goto('/notifications')
    await needsInbox(page)
    const payload = await getPayload(page)
    const direct = payload.classes!.filter((c) => c.category === 'direct').flatMap((c) => c.kinds.map((k) => k.id))
    await page.goto('/notifications?view=all&unread=1&category=direct')
    const oracle = await sql<Array<{ id: string }>>`
      select id::text from notifications
      where user_id = ${me.id}::uuid and archived_at is null and read_at is null and kind = any(${direct})
      order by created_at desc, id desc`
    await expect(page.locator('[data-center-list] [data-notification-row]')).toHaveCount(oracle.length)
    const shown = await page.locator('[data-center-list] [data-notification-row]').evaluateAll((els) => els.map((e) => e.getAttribute('data-notification-row')))
    expect(shown).toEqual(oracle.map((r) => r.id))
    await ctx.close()
  })

  test('130 rows page in, every one reachable once', async ({ browser }) => {
    const me = await person('paging')
    const t0 = Date.now()
    const rows: Seed[] = Array.from({ length: 130 }, (_, i) => ({ kind: 'mention', title: `Mention ${String(i + 1).padStart(3, '0')}`, href: '/home/inbox' }))
    const ids = await seed(me, rows, t0)
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.goto('/notifications')
    await needsInbox(page)
    const list = page.locator('[data-center-list] [data-notification-row]')
    await expect(list).toHaveCount(50)
    // Scroll to the end, as a person does: the sentinel loads the next page.
    // "Load more" is the fallback — it vanishes once the last page is in, so
    // a click on it is a short attempt, never a wait.
    for (let i = 0; i < 20 && (await list.count()) < 130; i++) {
      await list.last().scrollIntoViewIfNeeded()
      await page.locator('[data-center-more]').click({ timeout: 1_000 }).catch(() => {})
      await page.waitForTimeout(500)
    }
    await expect(list).toHaveCount(130)
    const shown = await list.evaluateAll((els) => els.map((e) => e.getAttribute('data-notification-row')))
    expect(new Set(shown).size).toBe(130)
    expect(new Set(shown)).toEqual(new Set(ids))
    await expect(page.locator('[data-center-more]')).toHaveCount(0)
    await ctx.close()
  })

  test('the keyboard: j k e u Enter ? x Shift+E', async ({ browser }) => {
    const me = await person('keys')
    const rows: Seed[] = Array.from({ length: 12 }, (_, i) => ({ kind: 'mention', title: `Key row ${i + 1}`, href: '/settings/profile' }))
    const ids = await seed(me, rows)
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.goto('/notifications')
    await needsInbox(page)
    const list = page.locator('[data-center-list] [data-notification-row]')
    await expect(list).toHaveCount(12)
    await expect(page.locator(`[data-notification-row="${ids[0]}"]`)).toHaveAttribute('data-focused', 'true')
    for (let i = 0; i < 3; i++) await page.keyboard.press('j')
    await expect(page.locator(`[data-notification-row="${ids[3]}"]`)).toHaveAttribute('data-focused', 'true')
    await page.keyboard.press('e')
    await expect(page.locator(`[data-notification-row="${ids[3]}"]`)).toHaveCount(0)
    await expect.poll(async () => (await sql`select archived_at from notifications where id = ${ids[3]!}::uuid`)[0]?.archived_at ?? null).not.toBeNull()
    // The focus stays in place: the 5th row is now the 4th.
    await expect(page.locator(`[data-notification-row="${ids[4]}"]`)).toHaveAttribute('data-focused', 'true')
    await page.keyboard.press('k')
    await expect(page.locator(`[data-notification-row="${ids[2]}"]`)).toHaveAttribute('data-focused', 'true')
    await page.keyboard.press('u')
    await expect(page.locator(`[data-notification-row="${ids[2]}"]`)).toHaveAttribute('data-unread', 'false')
    await page.keyboard.press('u')
    await expect(page.locator(`[data-notification-row="${ids[2]}"]`)).toHaveAttribute('data-unread', 'true')
    await page.keyboard.press('?')
    await expect(page.locator('[data-center-help]')).toBeVisible()
    await page.keyboard.press('Escape')
    await expect(page.locator('[data-center-help]')).toHaveCount(0)
    // x on two rows, Shift+E: both Done.
    await page.keyboard.press('x')
    await page.keyboard.press('j')
    await page.keyboard.press('x')
    await expect(page.locator('[data-center-selection]')).toHaveAttribute('data-center-selection', '2')
    await page.keyboard.press('Shift+E')
    await expect(page.locator(`[data-notification-row="${ids[2]}"]`)).toHaveCount(0)
    await expect(page.locator(`[data-notification-row="${ids[4]}"]`)).toHaveCount(0)
    await expect(list).toHaveCount(9)
    // Enter opens the focused row, in place.
    await page.evaluate(() => ((window as unknown as { __nc: number }).__nc = 1))
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(/\/settings\/profile$/)
    expect(await page.evaluate(() => (window as unknown as { __nc?: number }).__nc)).toBe(1)
    await ctx.close()
  })

  test('undo: Done in the bell and on the page comes back exactly, unread again', async ({ browser }) => {
    const other = await person('other')
    const me = await person('undo')
    const ids = await seed(me, bellRows(other))
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.goto('/home')
    await needsInbox(page)
    const state = async (id: string) => {
      const [r] = await sql<Array<{ archived: boolean; read: boolean }>>`select archived_at is not null as archived, read_at is not null as read from notifications where id = ${id}::uuid`
      return `${r?.archived}/${r?.read}`
    }
    // The bell: Done, then Undo — back in For you, unread, and badged.
    await openBell(page)
    await page.getByRole('button', { name: 'For you' }).click()
    await page.locator(`[data-notification-row="${ids[0]}"]`).hover()
    await page.locator(`[data-notification-row="${ids[0]}"] button[title="Done"]`).click()
    await expect(page.locator(`[data-notification-row="${ids[0]}"]`)).toHaveCount(0)
    await expect.poll(() => state(ids[0]!)).toBe('true/true')
    await page.locator('[data-bell-undo-button]').click()
    await expect.poll(() => state(ids[0]!)).toBe('false/false')
    await expect(page.locator(`[data-bell-list] [data-notification-row="${ids[0]}"]`)).toHaveCount(1)
    await expect(page.locator('[data-notify-badge]')).toHaveAttribute('data-notify-badge', '5')
    await page.keyboard.press('Escape')

    // The page: Shift+E (every row shown), then z — all back, each as it was.
    await sql`update notifications set read_at = now() where id = ${ids[2]!}::uuid`
    await page.goto('/notifications?view=all')
    const list = page.locator('[data-center-list] [data-notification-row]')
    await expect(list).toHaveCount(9)
    await page.keyboard.press('Shift+E')
    await expect(list).toHaveCount(0)
    await expect(page.locator('[data-center-undo]')).toContainText('9 notifications marked done')
    await expect.poll(async () => (await sql`select count(*)::int as n from notifications where user_id = ${me.id}::uuid and archived_at is not null`)[0]?.n).toBe(9)
    await page.keyboard.press('z')
    await expect(list).toHaveCount(9)
    await expect.poll(() => state(ids[2]!)).toBe('false/true')
    await expect.poll(() => state(ids[3]!)).toBe('false/false')
    await expect(page.locator('[data-center-undo]')).toHaveCount(0)
    await ctx.close()
  })

  test('the keyboard survives Caps Lock and a click on a select box', async ({ browser }) => {
    const me = await person('capslock')
    const rows: Seed[] = Array.from({ length: 6 }, (_, i) => ({ kind: 'mention', title: `Caps row ${i + 1}`, href: '/settings/profile' }))
    const ids = await seed(me, rows)
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.goto('/notifications')
    await needsInbox(page)
    const list = page.locator('[data-center-list] [data-notification-row]')
    await expect(list).toHaveCount(6)
    // Caps Lock: the browser sends 'E' with no Shift — the focused row only.
    await page.keyboard.press('J')
    await expect(page.locator(`[data-notification-row="${ids[1]}"]`)).toHaveAttribute('data-focused', 'true')
    await page.keyboard.press('E')
    await expect(page.locator(`[data-notification-row="${ids[1]}"]`)).toHaveCount(0)
    await expect(list).toHaveCount(5)
    // A click on a row's select box leaves the focus in a checkbox, and the
    // keyboard still works: e marks the selection done.
    await page.locator(`[data-notification-row="${ids[4]}"] input[type="checkbox"]`).click()
    await expect(page.locator('[data-center-selection]')).toHaveAttribute('data-center-selection', '1')
    await page.keyboard.press('e')
    await expect(page.locator(`[data-notification-row="${ids[4]}"]`)).toHaveCount(0)
    await expect(list).toHaveCount(4)
    await ctx.close()
  })

  test('Shift+E over more rows than one write takes marks every one done', async ({ browser }) => {
    test.setTimeout(600_000)
    const me = await person('bulk')
    const t0 = Date.now()
    const rows: Seed[] = Array.from({ length: 230 }, (_, i) => ({ kind: 'mention', title: `Bulk ${String(i + 1).padStart(3, '0')}`, href: '/home/inbox' }))
    await seed(me, rows, t0)
    const ctx = await contextFor(browser, me)
    const page = await ctx.newPage()
    await page.goto('/notifications')
    await needsInbox(page)
    const list = page.locator('[data-center-list] [data-notification-row]')
    for (let i = 0; i < 40 && (await list.count()) < 230; i++) {
      await list.last().scrollIntoViewIfNeeded()
      await page.locator('[data-center-more]').click({ timeout: 1_000 }).catch(() => {})
      await page.waitForTimeout(500)
    }
    await expect(list).toHaveCount(230)
    await page.keyboard.press('Shift+E')
    await expect
      .poll(async () => (await sql`select count(*)::int as n from notifications where user_id = ${me.id}::uuid and archived_at is not null`)[0]?.n, { timeout: 60_000 })
      .toBe(230)
    await expect(page.locator('[data-center-undo]')).toContainText('230 notifications marked done')
    await ctx.close()
  })
})
