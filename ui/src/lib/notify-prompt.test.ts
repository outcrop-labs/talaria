import { describe, expect, it, vi } from 'vitest'
import type { ShellNotificationStatus } from './desktop-shell'
import {
  MIN_LOADED_MS,
  SHELL_ASK_KEY,
  createShellAsk,
  shouldAsk,
  shouldAskShell,
  turnOnResult,
  type AskInput,
  type ShellAskEnv,
  type ShellAskInput,
  type ShellAskView,
} from './notify-prompt'

const NOW = Date.parse('2026-10-31T12:00:00.000Z')
const ok: AskInput = {
  permission: 'default',
  inShell: false,
  memory: { dismissedAt: null, count: 0, nextAskAt: null, granted: false },
  now: NOW,
  loadedAt: NOW - MIN_LOADED_MS,
  hasBeenActive: true,
  trigger: true,
}

describe('shouldAsk (the soft ask)', () => {
  it('asks when every condition holds', () => {
    expect(shouldAsk(ok)).toBe(true)
  })

  it('never asks on load: no trigger, no touch, or too soon', () => {
    expect(shouldAsk({ ...ok, trigger: false })).toBe(false)
    expect(shouldAsk({ ...ok, hasBeenActive: false })).toBe(false)
    expect(shouldAsk({ ...ok, loadedAt: NOW - MIN_LOADED_MS + 1 })).toBe(false)
  })

  it('asks only from the default state, and never in the shell', () => {
    for (const permission of ['granted', 'denied', 'insecure', 'unsupported'] as const) {
      expect(shouldAsk({ ...ok, permission })).toBe(false)
    }
    expect(shouldAsk({ ...ok, inShell: true })).toBe(false)
  })

  it('waits for the server record, honors nextAskAt, retires at three, stops on grant', () => {
    expect(shouldAsk({ ...ok, memory: undefined })).toBe(false)
    const later = new Date(NOW + 1000).toISOString()
    const earlier = new Date(NOW - 1000).toISOString()
    expect(shouldAsk({ ...ok, memory: { dismissedAt: earlier, count: 1, nextAskAt: later, granted: false } })).toBe(false)
    expect(shouldAsk({ ...ok, memory: { dismissedAt: earlier, count: 1, nextAskAt: earlier, granted: false } })).toBe(true)
    expect(shouldAsk({ ...ok, memory: { dismissedAt: earlier, count: 2, nextAskAt: earlier, granted: false } })).toBe(true)
    expect(shouldAsk({ ...ok, memory: { dismissedAt: earlier, count: 3, nextAskAt: null, granted: false } })).toBe(false)
    expect(shouldAsk({ ...ok, memory: { dismissedAt: null, count: 0, nextAskAt: null, granted: true } })).toBe(false)
  })
})

describe('turnOnResult', () => {
  it('reads a closed prompt as "try again", not as a dismissal', () => {
    expect(turnOnResult('granted')).toEqual({ kind: 'on' })
    expect(turnOnResult('default')).toEqual({
      kind: 'closed',
      message: "The browser's prompt closed without an answer — Turn on to try again.",
    })
    expect(turnOnResult('denied')).toEqual({ kind: 'blocked' })
    expect(turnOnResult('insecure').kind).toBe('unavailable')
    expect(turnOnResult('unsupported').kind).toBe('unavailable')
  })
})

// desktop-2: macOS refuses every banner until the person allows Talaria, and
// asks only when the shell requests it from a click. The launcher's card was
// the only such click outside Settings, and a person who resumes into an
// instance never sees it. The instance asks too — through a faked shell here.
describe('shouldAskShell (the shell asks, inside the instance)', () => {
  const okShell: ShellAskInput = {
    inShell: true,
    permission: 'not-determined',
    remembered: false,
    now: NOW,
    loadedAt: NOW - MIN_LOADED_MS,
    hasBeenActive: true,
  }

  it('asks while macOS has not, once the page was open a while and touched', () => {
    expect(shouldAskShell(okShell)).toBe(true)
    expect(shouldAskShell({ ...okShell, loadedAt: NOW - MIN_LOADED_MS + 1 })).toBe(false)
    expect(shouldAskShell({ ...okShell, hasBeenActive: false })).toBe(false)
  })

  it('never in a browser, never once decided, never after Not now, never on an older shell', () => {
    expect(shouldAskShell({ ...okShell, inShell: false })).toBe(false)
    for (const permission of ['granted', 'denied', 'unavailable', null, undefined] as const) {
      expect(shouldAskShell({ ...okShell, permission })).toBe(false)
    }
    expect(shouldAskShell({ ...okShell, remembered: true })).toBe(false)
  })
})

describe('createShellAsk (over a faked shell)', () => {
  const status = (permission: ShellNotificationStatus['permission'], over: Partial<ShellNotificationStatus> = {}) =>
    ({ backend: 'un', permission, pending: false, closeToBackground: true, background: 'menu-bar', ...over }) as ShellNotificationStatus

  function rig(over: Partial<ShellAskEnv> = {}) {
    const store = new Map<string, string>()
    let now = NOW
    let os: ShellNotificationStatus['permission'] = 'not-determined'
    let answer: ShellNotificationStatus['permission'] = 'granted'
    const views: ShellAskView[] = []
    const shell = {
      status: vi.fn(async () => status(os)),
      request: vi.fn(async () => {
        os = answer
        return status(os)
      }),
    }
    const ask = createShellAsk({
      inShell: () => true,
      status: shell.status,
      request: shell.request,
      memory: { get: () => store.get(SHELL_ASK_KEY) ?? null, set: (v) => void store.set(SHELL_ASK_KEY, v) },
      now: () => now,
      loadedAt: NOW,
      hasBeenActive: () => true,
      onView: (v) => views.push(v),
      ...over,
    })
    return {
      ask,
      shell,
      store,
      views,
      later: (ms: number) => (now += ms),
      setOs: (p: ShellNotificationStatus['permission']) => (os = p),
      answerWith: (p: ShellNotificationStatus['permission']) => (answer = p),
    }
  }

  it('waits, then shows; Allow runs the OS prompt and the card goes for good', async () => {
    const r = rig()
    expect(await r.ask.check()).toBe('later') // just loaded
    expect(r.ask.view().kind).toBe('hidden')
    r.later(MIN_LOADED_MS)
    expect(await r.ask.check()).toBe('shown')
    expect(r.ask.view()).toEqual({ kind: 'ask', os: 'macOS', busy: false, message: null })
    const allowing = r.ask.allow()
    expect(r.ask.view()).toMatchObject({ kind: 'ask', busy: true })
    await allowing
    expect(r.shell.request).toHaveBeenCalledTimes(1)
    expect(r.ask.view().kind).toBe('hidden')
    expect(r.store.get(SHELL_ASK_KEY)).toBe('answered')
    expect(await r.ask.check()).toBe('no')
  })

  it('a Don\'t Allow in the OS prompt says where to change it, and is not asked again', async () => {
    const r = rig()
    r.answerWith('denied')
    r.later(MIN_LOADED_MS)
    await r.ask.check()
    await r.ask.allow()
    expect(r.ask.view().kind).toBe('blocked')
    r.ask.close()
    expect(r.ask.view().kind).toBe('hidden')
    expect(await r.ask.check()).toBe('no')
  })

  it('Not now is remembered for this instance: a reload does not ask again', async () => {
    const r = rig()
    r.later(MIN_LOADED_MS)
    await r.ask.check()
    r.ask.notNow()
    expect(r.ask.view().kind).toBe('hidden')
    expect(r.store.get(SHELL_ASK_KEY)).toBe('dismissed')
    const reload = createShellAsk({
      inShell: () => true,
      status: r.shell.status,
      request: r.shell.request,
      memory: { get: () => r.store.get(SHELL_ASK_KEY) ?? null, set: () => {} },
      now: () => NOW + MIN_LOADED_MS * 2,
      loadedAt: NOW,
      hasBeenActive: () => true,
      onView: () => {},
    })
    expect(await reload.check()).toBe('no')
    expect(r.shell.request).not.toHaveBeenCalled()
  })

  it('an unanswered prompt or the shell\'s rate limit keeps the card, with a line', async () => {
    const r = rig()
    r.answerWith('not-determined')
    r.later(MIN_LOADED_MS)
    await r.ask.check()
    await r.ask.allow()
    expect(r.ask.view()).toMatchObject({ kind: 'ask', busy: false, message: expect.stringContaining('No answer yet') })
    r.shell.request.mockRejectedValueOnce('rate limited')
    await r.ask.allow()
    expect(r.ask.view()).toMatchObject({ kind: 'ask', busy: false, message: expect.stringContaining('Try again in a few seconds') })
    expect(r.store.has(SHELL_ASK_KEY)).toBe(false)
  })

  it('comes down when the question is answered elsewhere (the launcher, another instance)', async () => {
    const r = rig()
    r.later(MIN_LOADED_MS)
    await r.ask.check()
    expect(r.ask.view().kind).toBe('ask')
    r.setOs('granted')
    expect(await r.ask.check()).toBe('no')
    expect(r.ask.view().kind).toBe('hidden')
    expect(r.shell.request).not.toHaveBeenCalled()
  })

  it('shows nothing on Linux or Windows (granted, or no service), in a browser, or on an older shell', async () => {
    for (const p of ['granted', 'unavailable'] as const) {
      const r = rig()
      r.setOs(p)
      r.later(MIN_LOADED_MS)
      expect(await r.ask.check()).toBe('no')
      expect(r.views).toEqual([])
    }
    const browser = rig({ inShell: () => false })
    browser.later(MIN_LOADED_MS)
    expect(await browser.ask.check()).toBe('no')
    expect(browser.shell.status).not.toHaveBeenCalled()
    const older = rig({ status: async () => null })
    older.later(MIN_LOADED_MS)
    expect(await older.ask.check()).toBe('no')
    expect(older.views).toEqual([])
  })

  it('waits for a page the person has touched', async () => {
    let active = false
    const r = rig({ hasBeenActive: () => active })
    r.later(MIN_LOADED_MS)
    expect(await r.ask.check()).toBe('later')
    active = true
    expect(await r.ask.check()).toBe('shown')
  })
})
