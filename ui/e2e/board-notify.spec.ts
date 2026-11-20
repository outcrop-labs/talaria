// Board notifications in the browser (docs/notifications/producers.md "Each
// board", docs/user/boards.md "Notifications"): the bell beside a board's
// gear — value-faced, named "Board notifications: <value>", at least 24×24,
// readable in both Mercury themes — its one-click presets and its door to
// Board settings → Notifications (`?settings=notifications`); the tab's
// form (Custom events and scope, the board's own Alerts and Email, Use my
// default); Settings → Notifications → Boards (the default for every board
// and each board's state, with a quick change and Use my default); a viewer,
// whose modal holds only their own setting; and the ticket panel's "Added by
// … / Watch it myself". Every write is read back through the api and across
// a reload and a second browser, because the setting is the server's.
//
// The cheap halves — the names, glyphs and summaries for every preset ×
// cadence × source, the whole-setting write, which tabs a role sees — are
// pinned in lib/board-notify.test.ts and components/board/board-settings.test.ts,
// which the light CI ui job runs; this file is the browser proof, in full mode.
import { randomBytes } from 'node:crypto'
import { expect, test, type Browser, type BrowserContext, type Locator, type Page } from '@playwright/test'
import { Api, BASE_URL, RUN } from './support'

const THEMES = ['mercury', 'mercury-light'] as const

interface NotifyAnswer {
  effective: { preset: string; events: string[]; scope: string; cadence: string; channels: { alerts: boolean | null; email: boolean | null } }
  source: string
}

async function seedBoard(api: Api, name: string): Promise<{ boardId: string; taskId: string }> {
  const { board } = await api.post<{ board: { id: string } }>('/api/boards', { name: `${name} ${RUN}` })
  const { task } = await api.post<{ task: { id: string } }>(`/api/boards/${board.id}/tasks`, { title: 'A ticket to watch' })
  return { boardId: board.id, taskId: task.id }
}

const bellOf = (page: Page) => page.getByTestId('board-notify').getByRole('button').first()
const notifyOf = (api: Api, boardId: string) => api.get<NotifyAnswer>(`/api/boards/${boardId}/notify`)

type Rgb = readonly [number, number, number]
const rgb = (css: string): Rgb => {
  const p = (css.match(/rgba?\(([^)]+)\)/)?.[1] ?? '0,0,0').split(/[ ,/]+/).filter(Boolean).map(Number)
  return [p[0] ?? 0, p[1] ?? 0, p[2] ?? 0]
}
const luminance = ([r, g, b]: Rgb) => {
  const f = (c: number) => ((c /= 255) <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}
/** The glyph's colour against the first opaque surface behind it. */
async function glyphContrast(el: Locator): Promise<number> {
  const { fg, bg } = await el.evaluate((node) => {
    const svg = node.querySelector('svg') as SVGElement
    let n: Element | null = node
    let back = getComputedStyle(document.body).backgroundColor
    while (n) {
      const c = getComputedStyle(n).backgroundColor
      const a = c.match(/rgba\([^)]*,\s*([\d.]+)\)/)
      if (c !== 'transparent' && (!a || Number(a[1]) > 0.5)) {
        back = c
        break
      }
      n = n.parentElement
    }
    return { fg: getComputedStyle(svg).color, bg: back }
  })
  const [la, lb] = [luminance(rgb(fg)), luminance(rgb(bg))]
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
}

/** The bell: its name is its tooltip, it shows a value glyph (never the
 *  gear), its target is the board header's 32px, and it reads at 3:1. */
async function expectBell(page: Page, name: string, glyph: string) {
  const bell = bellOf(page)
  await expect(bell).toHaveAttribute('aria-label', name)
  await expect(bell).toHaveAttribute('title', name)
  await expect(bell.locator(`svg.lucide-${glyph}`)).toHaveCount(1)
  await expect(bell.locator('svg.lucide-settings')).toHaveCount(0)
  const box = await bell.boundingBox()
  expect(Math.round(box?.width ?? 0)).toBe(32)
  expect(Math.round(box?.height ?? 0)).toBe(32)
  await page.mouse.move(1, 1)
  await bell.evaluate((el) => (el as HTMLElement).blur())
  await expect.poll(() => glyphContrast(bell), { message: `${name} at rest` }).toBeGreaterThanOrEqual(3)
  return bell
}

/** A menu entry by its label. A checked one reads "✓ <label>" (the menu
 *  draws the check as text), and in the sans voice (Settings) a choice is a
 *  menuitemcheckbox rather than a menuitem. */
const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
const menuItem = (page: Page, label: string) => {
  const name = new RegExp('^(✓ )?' + escapeRe(label) + '$')
  return page.getByRole('menuitem', { name }).or(page.getByRole('menuitemcheckbox', { name }))
}

const pickFromBell = async (page: Page, item: string) => {
  await bellOf(page).click()
  await menuItem(page, item).click()
}

// Every chapter starts from the standard default for every board, whatever
// a chapter before it left behind.
test.beforeEach(async ({ request }) => {
  const res = await request.fetch('/api/notifications', { method: 'PATCH', data: { boards: null } })
  expect(res.ok(), `clearing the default for every board: ${res.status()}`).toBe(true)
})

for (const theme of THEMES) {
  test.describe(`board notifications — ${theme}`, () => {
    test.beforeEach(async ({ page }) => {
      await page.addInitScript((t) => localStorage.setItem('talaria-theme', t), theme)
    })

    test('the bell: a value, one click per preset, kept by the server across a reload and a second browser', async ({ page, request, browser }) => {
      const api = new Api(request)
      const { boardId } = await seedBoard(api, 'Bell')
      await page.goto(`/boards/${boardId}`)
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
      await expectBell(page, 'Board notifications: participating (your default)', 'bell')
      // FR4's gear stays beside it.
      await expect(page.getByRole('button', { name: 'Board settings', exact: true })).toHaveCount(1)

      await bellOf(page).click()
      for (const item of ['All activity', 'Participating', 'Mentions & assignments', 'Off', 'Custom…', 'Notification settings']) {
        await expect(menuItem(page, item)).toBeVisible()
      }
      // The value is checked: the menu says which one applies.
      await expect(page.getByRole('menuitem', { name: '✓ Participating', exact: true })).toBeVisible()
      await expect(menuItem(page, 'Notification settings').locator('svg.lucide-settings')).toHaveCount(1)
      await expect(menuItem(page, 'Use my default')).toHaveCount(0)
      await expect(page.locator('[data-board-notify-source]')).toHaveText('The standard default.')
      await page.keyboard.press('Escape')

      await pickFromBell(page, 'Mentions & assignments')
      await expectBell(page, 'Board notifications: mentions & assignments', 'at-sign')
      expect(await notifyOf(api, boardId)).toMatchObject({ source: 'board', effective: { preset: 'mentions' } })
      await pickFromBell(page, 'All activity')
      await expectBell(page, 'Board notifications: all activity', 'bell-ring')
      await pickFromBell(page, 'Off')
      await expectBell(page, 'Board notifications: off', 'bell-off')

      await page.reload()
      await expectBell(page, 'Board notifications: off', 'bell-off')
      const other: BrowserContext = await browser.newContext({ storageState: await page.context().storageState(), baseURL: BASE_URL })
      try {
        const second = await other.newPage()
        await second.addInitScript((t) => localStorage.setItem('talaria-theme', t), theme)
        await second.goto(`/boards/${boardId}`)
        await expect(bellOf(second)).toHaveAttribute('aria-label', 'Board notifications: off')
      } finally {
        await other.close()
      }

      // Use my default: the row goes, the board follows the default again.
      await pickFromBell(page, 'Use my default')
      await expectBell(page, 'Board notifications: participating (your default)', 'bell')
      expect((await notifyOf(api, boardId)).source).not.toBe('board')
    })

    test('the Notifications tab: Custom events and scope, the board’s own Alerts and Email, Use my default', async ({ page, request }) => {
      const api = new Api(request)
      const { boardId } = await seedBoard(api, 'Tab')
      await page.goto(`/boards/${boardId}`)
      await pickFromBell(page, 'Notification settings')
      await expect(page).toHaveURL(/[?&]settings=notifications/)
      const dialog = page.getByRole('dialog')
      await expect(dialog.getByText('Board settings').first()).toBeVisible()
      await expect(dialog.locator('[data-settings-tab="notifications"]')).toHaveAttribute('aria-pressed', 'true')
      // Every other tab is still there for an owner.
      await expect(dialog.locator('[data-settings-tab]')).toHaveCount(7)
      const form = dialog.locator('[data-board-notify-form="board"]')
      await expect(form.getByRole('radio', { name: /^Participating/ })).toBeChecked()

      await form.getByRole('radio', { name: /^Custom/ }).check()
      await expect(form).toHaveAttribute('data-preset', 'custom')
      const custom = form.locator('[data-board-notify-custom]')
      await expect(custom.getByRole('checkbox', { name: 'Assigned to me' })).toBeChecked()
      // New ticket needs the whole board: off under "Tickets I'm in".
      await expect(custom.getByRole('checkbox', { name: 'New ticket on the board' })).toBeDisabled()
      await custom.getByRole('button', { name: 'Every ticket', exact: true }).click()
      await expect(custom.getByRole('checkbox', { name: 'New ticket on the board' })).toBeEnabled()
      await custom.getByRole('checkbox', { name: 'New ticket on the board' }).check()
      await custom.getByRole('checkbox', { name: 'Comment or reply' }).uncheck()
      await expect.poll(async () => (await notifyOf(api, boardId)).effective).toMatchObject({
        preset: 'custom',
        scope: 'board',
        events: ['created', 'assigned', 'status', 'mention', 'completed'],
      })
      await expectBell(page, 'Board notifications: custom', 'bell-dot')

      // The board's own channels.
      await form.locator('[data-board-notify-channel="alerts"]').getByRole('button', { name: 'Off', exact: true }).click()
      await form.locator('[data-board-notify-channel="email"]').getByRole('button', { name: 'On', exact: true }).click()
      await expect.poll(async () => (await notifyOf(api, boardId)).effective.channels).toEqual({ alerts: false, email: true })
      await expect(form.locator('[data-board-notify-summary]')).toContainText('Alerts off, Email on for this board.')
      // Closed, a preset from the bell keeps them.
      await page.keyboard.press('Escape')
      await expect(page.getByRole('dialog')).toHaveCount(0)
      await pickFromBell(page, 'Mentions & assignments')
      await expect.poll(async () => (await notifyOf(api, boardId)).effective).toMatchObject({ preset: 'mentions', channels: { alerts: false, email: true } })

      // The deep link opens the tab on what the server holds.
      await page.goto(`/boards/${boardId}?settings=notifications`)
      await expect(page.getByRole('dialog').locator('[data-board-notify-form="board"]').getByRole('radio', { name: /^Mentions & assignments/ })).toBeChecked()
      await expect(page.getByRole('dialog').locator('[data-board-notify-channel="email"]').getByRole('button', { name: 'On', exact: true })).toHaveAttribute('aria-pressed', 'true')
      await page.getByRole('dialog').locator('[data-board-notify-reset]').click()
      await expect.poll(async () => (await notifyOf(api, boardId)).source).not.toBe('board')
      await expect(page.getByRole('dialog').locator('[data-board-notify-reset]')).toHaveCount(0)
    })
  })
}

test('Settings → Notifications → Boards: the default for every board, and each board’s state with a quick change', async ({ page, request }) => {
  const api = new Api(request)
  const a = await seedBoard(api, 'Settings A')
  const b = await seedBoard(api, 'Settings B')
  await api.put(`/api/boards/${b.boardId}/notify`, { preset: 'all' })
  try {
    await page.goto('/settings/notifications')
    const section = page.locator('[data-notify-boards]')
    await expect(section.getByRole('heading', { name: 'Boards', exact: true })).toBeVisible()
    // The board with its own setting is listed first.
    const rows = section.locator('[data-notify-board]')
    await expect(rows.first()).toHaveAttribute('data-notify-board', b.boardId)
    await expect(section.locator(`[data-notify-board="${b.boardId}"]`)).toContainText('Set for this board · all activity')

    // The default: Off, and every board that follows it says so.
    const def = section.locator('[data-board-notify-form="default"]')
    await def.getByRole('radio', { name: /^Off/ }).check()
    await expect(section.locator(`[data-notify-board="${a.boardId}"]`)).toHaveAttribute('data-preset', 'off')
    await expect(section.locator(`[data-notify-board="${b.boardId}"]`)).toHaveAttribute('data-preset', 'all')
    const listed = await api.get<{ default: NotifyAnswer }>('/api/notifications/boards')
    expect(listed.default).toMatchObject({ source: 'default', effective: { preset: 'off' } })

    // A quick change from the row, then Use my default.
    const rowA = section.locator(`[data-notify-board="${a.boardId}"]`)
    await rowA.getByRole('button', { name: /notifications: off$/ }).click()
    await menuItem(page, 'Mentions & assignments').click()
    await expect(rowA).toHaveAttribute('data-source', 'board')
    expect((await notifyOf(api, a.boardId)).effective.preset).toBe('mentions')
    await rowA.getByRole('button', { name: /notifications: mentions & assignments$/ }).click()
    await menuItem(page, 'Use my default').click()
    await expect(rowA).toHaveAttribute('data-source', 'default')

    // The row's door opens the board's own tab.
    await rowA.getByRole('button', { name: /notifications: off$/ }).click()
    await menuItem(page, 'Notification settings').click()
    await expect(page).toHaveURL(new RegExp(`/boards/${a.boardId}\\?settings=notifications`))
    await expect(page.getByRole('dialog').locator('[data-board-notify-form="board"]')).toBeVisible()
  } finally {
    await request.fetch('/api/notifications', { method: 'PATCH', data: { boards: null } })
  }
})

// ── A viewer ────────────────────────────────────────────────────────────────

const VIEWER = { sub: 'e2e:board-notify:viewer', email: 'e2e-board-notify-viewer@talaria.test', name: 'Notify Viewer' }

/** The board shared with a throwaway member as a viewer, and a browser
 *  signed in as them (a users row, and `sess:<sid>` in the stack's Redis). */
async function viewerOn(api: Api, browser: Browser, boardId: string): Promise<BrowserContext> {
  const dbUrl = process.env.E2E_DATABASE_URL
  const redisUrl = process.env.E2E_REDIS_URL
  if (!dbUrl || !redisUrl) throw new Error('E2E_DATABASE_URL and E2E_REDIS_URL must name the stack’s own Postgres and Redis (docs/E2E.md)')
  const { default: postgres } = await import('postgres')
  const { Redis } = await import('ioredis')
  const sql = postgres(dbUrl, { max: 1, onnotice: () => {} })
  const redis = new Redis(redisUrl, { maxRetriesPerRequest: 2 })
  const sid = `e2e-bn-${randomBytes(20).toString('hex')}`
  try {
    const [row] = await sql<Array<{ id: string }>>`
      insert into users (sub, email, name, role) values (${VIEWER.sub}, ${VIEWER.email}, ${VIEWER.name}, 'member')
      on conflict (sub) do update set role = 'member'
      returning id::text`
    if (!row) throw new Error('the viewer upsert returned no row')
    await redis.set(`sess:${sid}`, JSON.stringify({ id: row.id, ...VIEWER, picture: null, role: 'member', provider: 'password' }), 'EX', 3600)
  } finally {
    await sql.end({ timeout: 2 })
    redis.disconnect()
  }
  await api.post(`/api/boards/${boardId}/members`, { email: VIEWER.email, role: 'viewer' })
  const ctx = await browser.newContext({
    baseURL: BASE_URL,
    viewport: { width: 1440, height: 960 },
    storageState: {
      cookies: [{ name: 'talaria_session', value: sid, domain: new URL(BASE_URL).hostname, path: '/', expires: -1, httpOnly: true, secure: false, sameSite: 'Lax' }],
      origins: [],
    },
  })
  await ctx.route('**/api/push/subscribe', (r) => r.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' }))
  return ctx
}

test('a viewer: the bell and their own Notifications, no board settings; a watch someone added says so', async ({ request, browser }) => {
  const api = new Api(request)
  const { boardId, taskId } = await seedBoard(api, 'Viewer')
  const ctx = await viewerOn(api, browser, boardId)
  try {
    const page = await ctx.newPage()
    await page.goto(`/boards/${boardId}`)
    await expectBell(page, 'Board notifications: participating (your default)', 'bell')
    await expect(page.getByRole('button', { name: 'Board settings', exact: true })).toHaveCount(0)

    // Their own setting, through the bell's door: the modal is that setting.
    const ownersBefore = await notifyOf(api, boardId)
    await pickFromBell(page, 'Notification settings')
    const dialog = page.getByRole('dialog')
    await expect(dialog.getByText('Notification settings').first()).toBeVisible()
    await expect(dialog.locator('[data-settings-tab]')).toHaveCount(0)
    await dialog.locator('[data-board-notify-form="board"]').getByRole('radio', { name: /^Off/ }).check()
    await expectBell(page, 'Board notifications: off', 'bell-off')
    // Theirs only: the owner's setting for the board did not move.
    expect(await notifyOf(api, boardId)).toEqual(ownersBefore)
    await page.keyboard.press('Escape')

    // A link to a tab a viewer cannot see opens nothing.
    await page.goto(`/boards/${boardId}?settings=general`)
    await expect(bellOf(page)).toBeVisible()
    await expect(page.getByRole('dialog')).toHaveCount(0)

    // The owner makes the viewer watch the ticket: the panel names who, and
    // "Watch it myself" makes the watch theirs.
    await api.post(`/api/tasks/${taskId}/watchers`, { watcher: VIEWER.email })
    await page.goto(`/boards/${boardId}/${taskId}`)
    const watchers = page.locator('[data-watchers]')
    await expect(watchers.locator('[data-watch-added-by]')).toContainText('added by E2E Admin')
    await expect(watchers.locator('[data-watch-note]')).toContainText('E2E Admin added your watch.')
    await watchers.locator('[data-watch-myself]').click()
    await expect(watchers.locator('[data-watch-myself]')).toHaveCount(0)
    await expect(watchers.locator('[data-watch-note]')).toHaveCount(0)
    await expect(watchers.locator('[data-watch-toggle]')).toHaveText('Unwatch')
    const full = await api.get<{ watcherOrigins: Array<{ watcher: string; addedBy: unknown }> }>(`/api/tasks/${taskId}`)
    expect(full.watcherOrigins).toEqual([{ watcher: VIEWER.email, addedBy: null }])
  } finally {
    await ctx.close()
  }
})
