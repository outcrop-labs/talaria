// The conversation header's notification menu (components/chat/
// ChannelNotifyMenu.svelte): a level and a timed mute, written to the
// person's own membership row through PUT /api/channels/{id}/notify, and
// served back by GET /api/channels/{id} as `notify`. Each chapter makes its
// own channel through the suite's session, so it runs beside any other.
//
// The last chapter needs the stack's database (E2E_DATABASE_URL, as the
// global setup's session mint does): it takes the person off the channel
// behind the page's back, the way a removal by someone else would, and
// expects the menu's next write to be refused — shown, and no row made.
import { expect, test, type Page } from '@playwright/test'
import { Api } from './support'

interface Notify {
  level: 'all' | 'mentions' | 'none'
  mutedUntil: string | null
  settable: boolean
}

async function newChannel(api: Api): Promise<string> {
  const { channel } = await api.post<{ channel: { id: string } }>('/api/channels', {
    name: `np-menu-${Date.now().toString(36)}`,
  })
  return channel.id
}

const notifyOf = async (api: Api, id: string) => (await api.get<{ notify: Notify }>(`/api/channels/${id}`)).notify
const trigger = (page: Page) => page.getByTestId('channel-notify').getByRole('button').first()

async function choose(page: Page, item: string) {
  await trigger(page).click()
  await page.getByRole('menuitem', { name: item }).click()
}

test('the level and a timed mute persist across a reload and match the server row', async ({ page, request }) => {
  const api = new Api(request)
  const id = await newChannel(api)
  await page.goto(`/comms/channel/${id}`)
  await expect(trigger(page)).toHaveAttribute('data-level', 'mentions')

  // Nothing — and it survives a reload.
  await choose(page, 'Nothing')
  await expect(trigger(page)).toHaveAttribute('data-level', 'none')
  expect((await notifyOf(api, id)).level).toBe('none')
  await page.reload()
  await expect(trigger(page)).toHaveAttribute('data-level', 'none')

  // Mute for 8 hours: the header says until when, the server row agrees.
  const before = Date.now()
  await choose(page, 'Mute for 8 hours')
  await expect(page.getByTestId('channel-notify-muted')).toContainText('Muted until')
  await expect(trigger(page)).toHaveAttribute('data-muted', 'true')
  const muted = await notifyOf(api, id)
  expect(muted.mutedUntil).not.toBeNull()
  const at = Date.parse(muted.mutedUntil ?? '')
  expect(at).toBeGreaterThan(before + 8 * 3_600_000 - 120_000)
  expect(at).toBeLessThan(Date.now() + 8 * 3_600_000 + 120_000)
  await page.reload()
  await expect(page.getByTestId('channel-notify-muted')).toContainText('Muted until')

  // Unmute clears it, on the page and on the server, and stays cleared.
  await choose(page, 'Unmute')
  await expect(page.getByTestId('channel-notify-muted')).toHaveCount(0)
  await expect(trigger(page)).toHaveAttribute('data-muted', 'false')
  expect((await notifyOf(api, id)).mutedUntil).toBeNull()
  await page.reload()
  await expect(trigger(page)).toHaveAttribute('data-muted', 'false')
  await expect(trigger(page)).toHaveAttribute('data-level', 'none')

  // Back to every message.
  await choose(page, 'All messages')
  await expect(trigger(page)).toHaveAttribute('data-level', 'all')
  expect((await notifyOf(api, id)).level).toBe('all')
})

test('a person taken off the channel gets the refusal shown, and no row is created', async ({ page, request }) => {
  const dbUrl = process.env.E2E_DATABASE_URL
  test.skip(!dbUrl, 'needs E2E_DATABASE_URL to take the person off the channel behind the page')
  const api = new Api(request)
  const id = await newChannel(api)
  await page.goto(`/comms/channel/${id}`)
  await expect(trigger(page)).toHaveAttribute('data-level', 'mentions')

  const { default: postgres } = await import('postgres')
  const sql = postgres(dbUrl as string, { max: 1, onnotice: () => {} })
  try {
    // Removed by someone else while the page is open: no member row now.
    await sql`delete from channel_members where channel_id = ${id}::uuid`
    await choose(page, 'Nothing')
    const dialog = page.getByRole('dialog')
    await expect(dialog).toContainText('Notifications not changed')
    await expect(dialog).toContainText('forbidden')
    // The refusal made nothing: the route never inserts a membership row.
    const rows = await sql<Array<{ n: number }>>`select count(*)::int as n from channel_members where channel_id = ${id}::uuid`
    expect(rows[0]?.n).toBe(0)
  } finally {
    await sql.end({ timeout: 2 })
  }
})
