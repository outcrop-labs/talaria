// The settings cog (docs/UI-CONVENTIONS.md, "Settings cog"): every door into
// a settings surface in a view wears the one gear, says WHICH settings in its
// accessible name and tooltip, keeps a target of at least 24×24, and its glyph
// holds 3:1 against what it sits on — at rest and focused, in both Mercury
// themes. The two mixed menus in the board views are split: a saved view and
// a workchain each get a gear of their own, with Delete on that panel's
// danger line. Each chapter makes its own board or channel through the
// suite's session. The last chapter opens the board as a viewer: a throwaway
// member the suite shares it with, signed in through a session minted in the
// stack's own Redis (E2E_DATABASE_URL / E2E_REDIS_URL, docs/E2E.md).
//
// The cheap structural half (the label is the name and the tooltip, the size
// classes, the one gear) is pinned in components/ui/settings-cog.test.ts,
// which the light CI ui job runs on every pull request; this file is the
// browser proof, and runs in full mode.
import { randomBytes } from 'node:crypto'
import { expect, test, type Browser, type BrowserContext, type Locator, type Page } from '@playwright/test'
import { Api, BASE_URL, RUN } from './support'

const THEMES = ['mercury', 'mercury-light'] as const

interface Seeded {
  boardId: string
  viewId: string
  chainId: string
  channelId: string
  groupId: string
}

async function seed(api: Api): Promise<Seeded> {
  const { board } = await api.post<{ board: { id: string } }>('/api/boards', { name: `Cog ${RUN}` })
  await api.post(`/api/boards/${board.id}/tasks`, { title: 'A ticket for the list view' })
  const { view } = await api.post<{ view: { id: string } }>(`/api/boards/${board.id}/views`, {
    name: 'By priority',
    config: { view: 'list', group: 'priority' },
  })
  const { workchain } = await api.post<{ workchain: { id: string } }>(`/api/boards/${board.id}/workchains`, { name: 'Pipeline' })
  const { channel } = await api.post<{ channel: { id: string } }>('/api/channels', { name: `cog-${RUN}`, kind: 'channel', topic: null })
  const group = await api.post<{ channel: { id: string } }>('/api/channels', { name: `cog-group-${RUN}`, kind: 'group', topic: null })
  return { boardId: board.id, viewId: view.id, chainId: workchain.id, channelId: channel.id, groupId: group.channel.id }
}

type Rgb = readonly [number, number, number]
const rgb = (css: string): Rgb => {
  const p = (css.match(/rgba?\(([^)]+)\)/)?.[1] ?? '0,0,0').split(/[ ,/]+/).filter(Boolean).map(Number)
  return [p[0] ?? 0, p[1] ?? 0, p[2] ?? 0]
}
const luminance = ([r, g, b]: Rgb) => {
  const f = (c: number) => ((c /= 255) <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}
const contrast = (a: Rgb, b: Rgb) => {
  const [la, lb] = [luminance(a), luminance(b)]
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05)
}

/** The glyph's colour against the first opaque surface behind it. */
async function glyphContrast(door: Locator): Promise<number> {
  const { fg, bg } = await door.evaluate((el) => {
    const svg = el.querySelector('svg') as SVGElement
    let n: Element | null = el
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
  return contrast(rgb(fg), rgb(bg))
}

/** One icon-only settings door: name, tooltip, glyph, size, contrast. */
async function expectDoor(page: Page, name: string, size: number) {
  const door = page.getByRole('button', { name, exact: true })
  await expect(door).toHaveCount(1)
  await expect(door).toHaveAttribute('title', name)
  await expect(door.locator('svg.lucide-settings')).toHaveCount(1)
  await expect(door.locator('svg')).toHaveCount(1)
  const box = await door.boundingBox()
  expect(box?.width).toBeGreaterThanOrEqual(24)
  expect(Math.round(box?.width ?? 0)).toBe(size)
  expect(Math.round(box?.height ?? 0)).toBe(size)
  // At rest: pointer away, nothing focused. IconButton's colour transitions,
  // so the read is polled until it holds.
  await page.mouse.move(1, 1)
  await door.evaluate((el) => (el as HTMLElement).blur())
  await expect.poll(() => glyphContrast(door), { message: `${name} at rest` }).toBeGreaterThanOrEqual(3)
  const rest = await glyphContrast(door)
  // Focused: the dither field lights on focus, and the gear brightens with it.
  await door.focus()
  await expect.poll(() => glyphContrast(door), { message: `${name} focused` }).toBeGreaterThan(rest)
  expect(await glyphContrast(door)).toBeGreaterThanOrEqual(3)
  await door.evaluate((el) => (el as HTMLElement).blur())
  return door
}

for (const theme of THEMES) {
  test.describe(`settings cog — ${theme}`, () => {
    test.beforeEach(async ({ page }) => {
      await page.addInitScript((t) => localStorage.setItem('talaria-theme', t), theme)
    })

    test('the board and its list view: one gear each, scoped, 32 and 28px, readable', async ({ page, request }) => {
      const s = await seed(new Api(request))
      await page.goto(`/boards/${s.boardId}`)
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
      const board = await expectDoor(page, 'Board settings', 32)
      await expect(page.locator('svg.lucide-settings-2')).toHaveCount(0)
      await board.click()
      await expect(page.getByRole('dialog').getByText('Board settings').first()).toBeVisible()
      await page.keyboard.press('Escape')

      await page.goto(`/boards/${s.boardId}?view=list`)
      const cols = await expectDoor(page, 'Column settings', 28)
      await expect(page.locator('svg.lucide-sliders-horizontal')).toHaveCount(0)
      // The 28px gear does not grow the header row past the other cells.
      const row = await page.locator('table thead tr').first().boundingBox()
      expect(row?.height ?? 99).toBeLessThanOrEqual(34)
      await cols.click()
      await expect(page.getByText('Columns: drag to reorder')).toBeVisible()
      await expect(cols).toHaveAttribute('aria-expanded', 'true')
    })

    test('a saved view: its own gear, name, update and Delete on the danger line; right-click still works', async ({ page, request }) => {
      const s = await seed(new Api(request))
      await page.goto(`/boards/${s.boardId}`)
      const tab = page.getByRole('button', { name: 'By priority', exact: true })
      await expect(page.getByRole('button', { name: 'View settings', exact: true })).toHaveCount(0)
      await tab.click()
      const door = await expectDoor(page, 'View settings', 28)
      await door.click()
      const panel = page.locator('[data-view-settings]')
      await expect(panel.locator('input')).toHaveValue('By priority')
      await expect(panel.getByRole('button', { name: 'Update to current filters' })).toBeDisabled()
      await expect(panel.getByRole('button', { name: 'Delete view' })).toBeVisible()
      await page.keyboard.press('Escape')
      // A filter change un-highlights the tab; the gear stays with the view,
      // and now there is something to update.
      await page.getByPlaceholder('Search').fill('ticket')
      await page.getByRole('button', { name: 'View settings', exact: true }).click()
      await expect(panel.getByRole('button', { name: 'Update to current filters' })).toBeEnabled()
      await page.keyboard.press('Escape')
      await tab.click({ button: 'right' })
      for (const item of ['Update to current filters', 'Rename', 'Delete view']) {
        await expect(page.getByRole('menuitem', { name: item })).toBeVisible()
      }
    })

    test('a workchain: the switcher switches, the gear holds name, Paused and Delete', async ({ page, request }) => {
      const s = await seed(new Api(request))
      await page.goto(`/boards/${s.boardId}?view=workchains&chain=${s.chainId}`)
      const door = await expectDoor(page, 'Workchain settings', 28)
      await page.getByTitle('Switch or create workchains').click()
      await expect(page.getByRole('menuitem', { name: 'New workchain' })).toBeVisible()
      await expect(page.getByRole('menuitem', { name: /Rename|Pause|Resume|Delete/ })).toHaveCount(0)
      await page.keyboard.press('Escape')
      await door.click()
      const panel = page.locator('[data-workchain-settings]')
      await expect(panel.locator('input')).toHaveValue('Pipeline')
      await panel.getByRole('switch').click()
      await expect(page.getByText('paused', { exact: true })).toBeVisible()
      await panel.getByRole('switch').click()
      await expect(page.getByText('paused', { exact: true })).toHaveCount(0)
      await expect(panel.getByRole('button', { name: 'Delete workchain' })).toBeVisible()
    })

    test('comms: the channel and group doors, the value-faced notify control, and the bell footer', async ({ page, request }) => {
      const s = await seed(new Api(request))
      await page.goto(`/comms/channel/${s.channelId}`)
      await expectDoor(page, 'Channel settings', 28)
      const notify = page.getByTestId('channel-notify').getByRole('button').first()
      await expect(notify).toHaveAttribute('aria-label', /^Channel notifications: /)
      expect(await notify.getAttribute('title')).toBe(await notify.getAttribute('aria-label'))
      await expect(notify.locator('svg.lucide-settings')).toHaveCount(0)
      await notify.click()
      const entry = page.getByRole('menuitem', { name: 'Notification settings' })
      await expect(entry.locator('svg.lucide-settings')).toHaveCount(1)
      await entry.click()
      await expect(page).toHaveURL(/\/settings\/notifications$/)

      await page.goto(`/comms/channel/${s.groupId}`)
      await expectDoor(page, 'Group settings', 28)

      await page.goto(`/boards/${s.boardId}`)
      await page.locator('[data-notify-badge]').first().click()
      const link = page.locator('[data-bell-settings]')
      await expect(link).toHaveText('Notification settings')
      await expect(link).toHaveAttribute('href', /\/settings\/notifications$/)
      await expect(link.locator('svg.lucide-settings')).toHaveCount(1)
      expect((await link.boundingBox())?.height ?? 0).toBeGreaterThanOrEqual(24)
    })
  })
}

// ── A viewer ────────────────────────────────────────────────────────────────

const VIEWER = { sub: 'e2e:cog:viewer', email: 'e2e-cog-viewer@talaria.test', name: 'Cog Viewer' }

/** The board shared with a throwaway member as a viewer, and a browser
 *  signed in as them — the session minted the way global-setup mints the
 *  suite's (a users row, and `sess:<sid>` in Redis). */
async function viewerOn(api: Api, browser: Browser, boardId: string): Promise<BrowserContext> {
  const dbUrl = process.env.E2E_DATABASE_URL
  const redisUrl = process.env.E2E_REDIS_URL
  if (!dbUrl || !redisUrl) throw new Error('E2E_DATABASE_URL and E2E_REDIS_URL must name the stack’s own Postgres and Redis (docs/E2E.md)')
  const { default: postgres } = await import('postgres')
  const { Redis } = await import('ioredis')
  const sql = postgres(dbUrl, { max: 1, onnotice: () => {} })
  const redis = new Redis(redisUrl, { maxRetriesPerRequest: 2 })
  const sid = `e2e-cog-${randomBytes(20).toString('hex')}`
  try {
    const [row] = await sql<Array<{ id: string }>>`
      insert into users (sub, email, name, role) values (${VIEWER.sub}, ${VIEWER.email}, ${VIEWER.name}, 'member')
      on conflict (sub) do update set role = 'member'
      returning id::text`
    if (!row) throw new Error('the viewer upsert returned no row')
    const session = { id: row.id, ...VIEWER, picture: null, role: 'member', provider: 'password' }
    await redis.set(`sess:${sid}`, JSON.stringify(session), 'EX', 3600)
  } finally {
    await sql.end({ timeout: 2 })
    redis.disconnect()
  }
  await api.post(`/api/boards/${boardId}/members`, { email: VIEWER.email, role: 'viewer' })
  const { members } = await api.get<{ members: Array<{ email: string | null; role: string }> }>(`/api/boards/${boardId}/members`)
  expect(members.find((m) => m.email === VIEWER.email)?.role, 'the viewer is on the board as a viewer').toBe('viewer')
  const ctx = await browser.newContext({
    baseURL: BASE_URL,
    viewport: { width: 1440, height: 960 },
    storageState: {
      cookies: [{ name: 'talaria_session', value: sid, domain: new URL(BASE_URL).hostname, path: '/', expires: -1, httpOnly: true, secure: false, sameSite: 'Lax' }],
      origins: [],
    },
  })
  // The server never learns of a push subscription from a test browser.
  await ctx.route('**/api/push/subscribe', (r) => r.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' }))
  return ctx
}

// Saving, renaming or deleting a view, every workchain write and the board's
// settings are owner/editor writes the api refuses a viewer (403), so a viewer
// is offered none of those doors. The list's columns are a per-person view
// setting, and stay. The theme does not change who sees a door: one pass.
test('a viewer: no board, view or workchain settings door, no saved-view menu; column settings stay', async ({ request, browser }) => {
  const s = await seed(new Api(request))
  const ctx = await viewerOn(new Api(request), browser, s.boardId)
  try {
    const page = await ctx.newPage()
    await page.goto(`/boards/${s.boardId}`)
    // The saved-view tabs render only once the board (and the viewer's role
    // on it) has loaded, so every count of zero below is read after that.
    const tab = page.getByRole('button', { name: 'By priority', exact: true })
    await expect(tab).toBeVisible()
    await expect(page.getByRole('button', { name: 'Board settings', exact: true })).toHaveCount(0)
    await tab.click()
    await expect(page).toHaveURL(new RegExp(`[?&]v=${s.viewId}`))
    await expect(page.getByRole('button', { name: 'View settings', exact: true })).toHaveCount(0)
    await tab.click({ button: 'right' })
    for (const item of ['Update to current filters', 'Rename', 'Delete view']) {
      await expect(page.getByRole('menuitem', { name: item })).toHaveCount(0)
    }
    // The list view this saved view opened: its columns stay the viewer's own.
    await expectDoor(page, 'Column settings', 28)

    await page.goto(`/boards/${s.boardId}?view=workchains&chain=${s.chainId}`)
    await expect(page.getByTitle('Switch or create workchains')).toBeVisible()
    await expect(page.getByRole('button', { name: 'Workchain settings', exact: true })).toHaveCount(0)
  } finally {
    await ctx.close()
  }
})
