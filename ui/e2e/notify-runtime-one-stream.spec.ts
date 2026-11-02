// The notification runtime's one-stream rule (docs/notifications/web-runtime.md):
// however many Talaria tabs a browser has open, it holds ONE `/api/me/events`
// — the leader tab's, rebroadcast to the others on a BroadcastChannel — and
// when the leader goes, the next tab opens it within a couple of seconds.
// Counted the way the browser sees it: every EventSource any tab constructs.
import { expect, test, type Page } from '@playwright/test'

const COUNT = `(() => {
  const Real = window.EventSource
  window.__userStreams = 0
  window.EventSource = class extends Real {
    constructor(url, init) {
      super(url, init)
      if (/\\/api\\/me\\/events/.test(String(url))) window.__userStreams++
    }
  }
})()`

type Probe = Window & { __userStreams?: number; __talariaNotify?: { hubRole?: () => string | null } }

const role = (p: Page) => p.evaluate(() => (window as Probe).__talariaNotify?.hubRole?.() ?? null)
const streams = (p: Page) => p.evaluate(() => (window as Probe).__userStreams ?? 0)

test('one /api/me/events per browser — and the next tab takes over when the leader closes', async ({ context }) => {
  await context.addInitScript({ content: COUNT })
  const pages: Page[] = []
  for (const path of ['/', '/boards', '/']) {
    const p = await context.newPage()
    await p.goto(path)
    pages.push(p)
  }
  // Every tab settles into a role: one leader, the rest following it (none
  // falls back to its own stream, which a follower does only when no hub
  // answers within 2.5 s).
  await expect
    .poll(async () => (await Promise.all(pages.map(role))).sort().join(','), { timeout: 30_000 })
    .toBe('follower,follower,leader')
  await pages[0]!.waitForTimeout(3_000)
  expect((await Promise.all(pages.map(streams))).reduce((a, b) => a + b, 0)).toBe(1)
  expect((await Promise.all(pages.map(role))).sort().join(',')).toBe('follower,follower,leader')

  const roles = await Promise.all(pages.map(role))
  const leader = pages[roles.indexOf('leader')]!
  const rest = pages.filter((p) => p !== leader)
  const before = await Promise.all(rest.map(streams))
  await leader.close()
  await expect
    .poll(async () => (await Promise.all(rest.map(role))).sort().join(','), { timeout: 5_000 })
    .toBe('follower,leader')
  const after = await Promise.all(rest.map(streams))
  expect(after.reduce((a, b) => a + b, 0) - before.reduce((a, b) => a + b, 0)).toBe(1)
  for (const p of rest) await p.close()
})
