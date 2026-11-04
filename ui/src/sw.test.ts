// The service worker's own rules, run in the worker's own code: sw.ts is
// loaded against a fake ServiceWorkerGlobalScope, and its handlers are
// dispatched the way the browser would. The page re-validates every href it
// is asked to open, so a page-level check can never see the worker's rule —
// that is how a dot-segment href once walked a window that did not answer
// off the instance. These are the two paths no page sees: no window at all
// (openWindow), and a window that does not answer (navigate).
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { safeInAppHref } from './lib/notify-open'

const ORIGIN = 'https://talaria.example.com'
const ROW = '1a5bba79-835d-495f-bd4e-fed9df6c701d'

type Handler = (event: unknown) => void
interface FakeWindow {
  url: string
  focused: boolean
  visibilityState: 'visible' | 'hidden'
  focus: () => Promise<FakeWindow>
  navigate: (url: string) => Promise<FakeWindow | null>
  postMessage: (msg: unknown, ports?: MessagePort[]) => void
}

const handlers = new Map<string, Handler>()
const opened: string[] = []
const navigated: string[] = []
const shown: Array<{ title: string; tag?: string; silent?: boolean }> = []
const fetches: Array<{ url: string; body?: string }> = []
let windows: FakeWindow[] = []
/** The banners on screen, as getNotifications answers them. */
let onScreen: Array<{ tag: string; title?: string; body?: string; data: { v?: string } }> = []

/** The bannered store (IndexedDB, which node lacks): a map behind just the
 *  calls sw.ts makes, each answering on a later microtask, as IDB does. */
const idb = new Map<string, { v: string; at: number }>()
function idbRequest<T>(result: () => T) {
  const r: { result?: T; onsuccess?: () => void; onerror?: () => void } = {}
  queueMicrotask(() => {
    r.result = result()
    r.onsuccess?.()
  })
  return r
}
const fakeIndexedDB = {
  open: () => {
    const db = {
      objectStoreNames: { contains: () => true },
      createObjectStore: () => {},
      transaction: () => ({
        objectStore: () => ({
          get: (k: string) => idbRequest(() => idb.get(k)),
          put: (v: { v: string; at: number }, k: string) => idbRequest(() => void idb.set(k, v)),
        }),
      }),
    }
    return idbRequest(() => db)
  },
}

/** A window: `answer` true acks "open this" (the app), false never answers
 *  (an older build, a frozen tab, a non-app page of the origin), 'throw'
 *  refuses the message outright. */
function win(url: string, answer: boolean | 'throw', received: unknown[] = []): FakeWindow {
  const w: FakeWindow = {
    url,
    focused: false,
    visibilityState: 'hidden',
    focus: async () => w,
    navigate: async (to: string) => {
      navigated.push(to)
      return w
    },
    postMessage: (msg, ports) => {
      received.push(msg)
      if (answer === 'throw') throw new Error('DataCloneError')
      if (answer) ports?.[0]?.postMessage({ type: 'talaria:notification-opened' })
    },
  }
  return w
}

async function click(data: Record<string, unknown>): Promise<void> {
  const pending: Promise<unknown>[] = []
  handlers.get('notificationclick')!({
    notification: { data, close: () => {} },
    waitUntil: (p: Promise<unknown>) => pending.push(p),
  })
  await Promise.all(pending)
}

async function push(note: Record<string, unknown>): Promise<void> {
  const pending: Promise<unknown>[] = []
  handlers.get('push')!({ data: { json: () => note }, waitUntil: (p: Promise<unknown>) => pending.push(p) })
  await Promise.all(pending)
}

beforeAll(async () => {
  const scope = {
    location: { origin: ORIGIN },
    addEventListener: (type: string, fn: Handler) => handlers.set(type, fn),
    skipWaiting: async () => {},
    clients: {
      claim: async () => {},
      matchAll: async () => windows,
      openWindow: async (url: string) => {
        opened.push(url)
        return null
      },
    },
    registration: {
      showNotification: async (title: string, opts: { tag?: string; body?: string; silent?: boolean; data?: { v?: string } }) => {
        shown.push(opts?.silent ? { title, tag: opts?.tag, silent: true } : { title, tag: opts?.tag })
        if (opts?.tag)
          onScreen = [
            ...onScreen.filter((n) => n.tag !== opts.tag),
            { tag: opts.tag, title, body: opts.body, data: opts.data ?? {} },
          ]
      },
      getNotifications: async (o?: { tag?: string }) => onScreen.filter((n) => !o?.tag || n.tag === o.tag),
    },
  }
  vi.stubGlobal('self', scope)
  vi.stubGlobal('indexedDB', fakeIndexedDB)
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: { body?: string }) => {
      fetches.push({ url, body: init?.body })
      return new Response('{}')
    }),
  )
  // sw.ts is a classic worker script, not a module: it exports nothing, and
  // importing it only runs it — against the fake scope above.
  const entry = './sw'
  await import(/* @vite-ignore */ entry)
})

afterEach(() => {
  windows = []
  onScreen = []
  idb.clear()
  opened.length = 0
  navigated.length = 0
  shown.length = 0
  fetches.length = 0
})

// The hrefs that must never leave the instance, by the worker's own rule:
// the plain foreign forms and every dot-segment spelling that resolves to
// the path `//evil.example`. The same dot table as talaria-notify's
// `openable_href_refuses_dot_segments_in_every_spelling`.
const EVIL = [
  'https://evil.example',
  '//evil.example',
  '/\\evil.example',
  'javascript:alert(1)',
  '/..//evil.example',
  '/.//evil.example',
  '/a/..//evil.example',
  '/%2e%2e//evil.example',
  '/%2E%2e//evil.example',
  '/.%2e//evil.example',
  '/%2e.//evil.example',
  '/%2e//evil.example',
  '/a/b/../../..//evil.example',
]
const offInstance = (url: string) => new URL(url, ORIGIN).origin !== ORIGIN

describe('sw notificationclick — no Talaria window (openWindow)', () => {
  it('opens home, never another host, for every foreign or dot-segment href', async () => {
    for (const href of EVIL) {
      opened.length = 0
      await click({ id: ROW, href })
      expect(opened, href).toEqual(['/'])
      expect(opened.some(offInstance), href).toBe(false)
    }
  })

  it('opens the path itself when it is a path on this origin', async () => {
    await click({ id: ROW, href: '/boards/b/t?x=1#c' })
    expect(opened).toEqual(['/boards/b/t?x=1#c'])
  })
})

describe('sw notificationclick — a window that does not answer (navigate)', () => {
  it('never navigates it for a foreign or dot-segment href: it stays where it is', async () => {
    for (const href of EVIL) {
      navigated.length = 0
      windows = [win(`${ORIGIN}/icons/notify-192.png`, 'throw')]
      await click({ id: ROW, href })
      expect(navigated, href).toEqual([])
      expect(opened, href).toEqual([])
    }
  })

  it('waits out the one-second ack, then navigates to a valid path on this origin', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    try {
      windows = [win(`${ORIGIN}/home`, false)]
      const done = click({ id: ROW, href: '/comms/channel/c' })
      await vi.advanceTimersByTimeAsync(1100)
      await done
      expect(navigated).toEqual([`${ORIGIN}/comms/channel/c`])
      // …and the same silent window with the reviewer's live case stays put.
      navigated.length = 0
      const evil = click({ id: ROW, href: '/..//evil.example' })
      await vi.advanceTimersByTimeAsync(1100)
      await evil
      expect(navigated).toEqual([])
    } finally {
      vi.useRealTimers()
    }
  })

  it('an empty href (the confirmation banner) neither navigates nor reads anything', async () => {
    windows = [win(`${ORIGIN}/boards`, 'throw')]
    await click({ id: '', href: '' })
    expect(navigated).toEqual([])
    expect(fetches).toEqual([])
  })
})

describe('sw notificationclick — the app answers', () => {
  it('hands the page only a checked path, and reads only a row id', async () => {
    const got: unknown[] = []
    windows = [win(`${ORIGIN}/home`, true, got)]
    await click({ id: ROW, href: '/..//evil.example' })
    await click({ id: 'talaria-summary', href: '', summary: true })
    expect(got).toEqual([
      { type: 'talaria:notification-open', id: ROW, href: '' },
      { type: 'talaria:notification-open', id: 'talaria-summary', href: '', summary: true },
    ])
    expect(navigated).toEqual([])
    // One PUT, for the row — never for the summary's tag (a 400).
    expect(fetches.map((f) => f.body)).toEqual([JSON.stringify({ ids: [ROW] })])
  })
})

describe("the worker's inlined rule is the page's rule", () => {
  it('agrees with safeInAppHref on every case', async () => {
    const cases = [...EVIL, '/', '/boards/b/t', '/.../x', '/..a//x', '/x?next=/..//evil.example', '/%2e%2ea/x', '', '  /inbox  ']
    for (const href of cases) {
      opened.length = 0
      await click({ id: ROW, href })
      expect(opened[0], href).toBe(safeInAppHref(href, ORIGIN) ?? '/')
    }
  })
})

describe('sw push', () => {
  it('shows nothing while a Talaria window is focused and visible', async () => {
    const w = win(`${ORIGIN}/home`, true)
    w.focused = true
    w.visibilityState = 'visible'
    windows = [w]
    await push({ id: ROW, title: 'Ada mentioned you', href: '/boards', createdAt: '2026-10-31T12:00:00.000Z' })
    expect(shown).toEqual([])
  })

  it('always shows a per-device test push, even over a focused visible window', async () => {
    const w = win(`${ORIGIN}/home`, true)
    w.focused = true
    w.visibilityState = 'visible'
    windows = [w]
    await push({ id: ROW, kind: 'test', title: 'Test notification', href: '', createdAt: '2026-10-31T12:00:00.000Z' })
    expect(shown).toEqual([{ title: 'Test notification', tag: ROW }])
    // …and an ordinary row still is not.
    shown.length = 0
    await push({ id: ROW, kind: 'dm', title: 'Bob', href: '/comms', createdAt: '2026-10-31T12:00:01.000Z' })
    expect(shown).toEqual([])
  })

  it('with no window, shows an unseen row once, under its own tag', async () => {
    await push({ id: ROW, title: 'Ada mentioned you', href: '/boards', createdAt: '2026-10-31T12:00:00.000Z' })
    expect(shown).toEqual([{ title: 'Ada mentioned you', tag: ROW }])
  })
})

// Two pushes of one row filed in quick succession (a DM row refreshed by a
// second message) are separate sends, and can arrive out of order.
describe('sw push — an out-of-order older push neither shows nor replaces the newer', () => {
  const NEWER = '2026-10-31T12:00:05.000Z'
  const OLDER = '2026-10-31T12:00:01.000Z'

  it('drops a push older than the version already bannered under its tag', async () => {
    await push({ id: ROW, title: 'Bob: second line', href: '/comms', renotify: true, createdAt: NEWER })
    expect(shown).toEqual([{ title: 'Bob: second line', tag: ROW }])
    await push({ id: ROW, title: 'Bob: first line', href: '/comms', renotify: true, createdAt: OLDER })
    expect(shown).toEqual([{ title: 'Bob: second line', tag: ROW }])
    expect(idb.get(ROW)?.v).toBe(NEWER)
    expect(onScreen.map((n) => n.data.v)).toEqual([NEWER])
  })

  it('a newer one still shows, replacing its own older copy', async () => {
    await push({ id: ROW, title: 'Bob: first line', href: '/comms', createdAt: OLDER })
    await push({ id: ROW, title: 'Bob: second line', href: '/comms', renotify: true, createdAt: NEWER })
    expect(shown.map((x) => x.title)).toEqual(['Bob: first line', 'Bob: second line'])
    expect(idb.get(ROW)?.v).toBe(NEWER)
  })

  it('the copy on screen says it too, when the store cannot', async () => {
    onScreen = [{ tag: ROW, data: { v: NEWER } }]
    await push({ id: ROW, title: 'Bob: first line', href: '/comms', renotify: true, createdAt: OLDER })
    expect(shown).toEqual([])
  })

  it('on WebKit too, where nothing else is skipped', async () => {
    const real = globalThis.navigator
    vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15' })
    try {
      idb.set(ROW, { v: NEWER, at: 1 })
      await push({ id: ROW, title: 'Bob: first line', href: '/comms', renotify: true, createdAt: OLDER })
      expect(shown).toEqual([])
      // …while WebKit still shows a repeat of the same version, as before.
      await push({ id: ROW, title: 'Bob: second line', href: '/comms', createdAt: NEWER })
      expect(shown).toEqual([{ title: 'Bob: second line', tag: ROW }])
    } finally {
      vi.stubGlobal('navigator', real)
    }
  })

  it('on WebKit, the older push shows the newer copy again, silently, so the push is never silent', async () => {
    const real = globalThis.navigator
    vi.stubGlobal('navigator', { userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_5) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15' })
    try {
      await push({ id: ROW, title: 'Bob: second line', body: 'and more', href: '/comms', renotify: true, createdAt: NEWER })
      await push({ id: ROW, title: 'Bob: first line', href: '/comms', renotify: true, createdAt: OLDER })
      expect(shown).toEqual([
        { title: 'Bob: second line', tag: ROW },
        { title: 'Bob: second line', tag: ROW, silent: true },
      ])
      expect(onScreen).toEqual([{ tag: ROW, title: 'Bob: second line', body: 'and more', data: expect.objectContaining({ v: NEWER }) }])
      expect(idb.get(ROW)?.v).toBe(NEWER)
    } finally {
      vi.stubGlobal('navigator', real)
    }
  })

  it('two pushes delivered together, newest first: they are handled one at a time, and the newer stays', async () => {
    await Promise.all([
      push({ id: ROW, title: 'Bob: second line', href: '/comms', renotify: true, createdAt: NEWER }),
      push({ id: ROW, title: 'Bob: first line', href: '/comms', renotify: true, createdAt: OLDER }),
    ])
    expect(shown).toEqual([{ title: 'Bob: second line', tag: ROW }])
    expect(onScreen.map((n) => n.data.v)).toEqual([NEWER])
  })
})
