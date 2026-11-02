// The in-context soft ask: WHEN Talaria invites a person to turn desktop
// alerts on. Pure, so every rule is pinned by a test.
//
// The browser's own permission prompt is a one-shot: dismissed about three
// times, Chrome embargoes the origin, and a "Block" is near-permanent. So the
// native prompt only ever runs behind a click on OUR card (rule 1 of
// browser-notify.ts), and our card only shows when there is a reason the
// person can see: a notification just landed here, in front of them, or came
// in while they were away from the tab. Never on load — an ask with no reason
// is the one people reflex-dismiss.
//
// The card's memory is the account's (`prompt` on GET /api/notifications,
// written by PATCH {prompt}): the first "Not now" waits 7 days, the second 30,
// and after the third the card is retired for good. The bell's own row stays,
// so turning alerts on later is always one click away.
//
// INSIDE THE DESKTOP SHELL the browser's prompt never runs: the shell posts
// through the OS, and the OS grant is the shell's. macOS is the one OS that
// asks — and only when the app requests it, which the shell does from a
// click alone. The launcher's card and Settings → Desktop are two such
// clicks, but a person who resumes straight into an instance (every upgrade
// from 0.11, every relaunch after the first add) sees neither, and every
// banner is refused while macOS has not asked. So the instance asks too:
// `shouldAskShell` and `createShellAsk` below, the card in
// DesktopNotifyPermission.svelte.

import type { PermissionState } from '@/lib/browser-notify'
import type { ShellNotificationStatus } from '@/lib/desktop-shell'

/** The server's record of the soft ask, as GET /api/notifications serves it. */
export interface PromptMemory {
  dismissedAt: string | null
  count: number
  nextAskAt: string | null
  granted: boolean
}

/** Dismissals after which the card never shows again (the server's cap). */
export const PROMPT_MAX_DISMISSALS = 3

/** The page must have been open this long before the card may show — a
 *  person who just arrived is reading, not deciding about alerts. */
export const MIN_LOADED_MS = 10_000

export interface AskInput {
  permission: PermissionState
  inShell: boolean
  /** Undefined while the server's record has not loaded: no ask. */
  memory: PromptMemory | undefined
  now: number
  /** When this page loaded (ms epoch). */
  loadedAt: number
  /** `navigator.userActivation.hasBeenActive`: the person has clicked or
   *  typed here. A page nobody has touched is not one to interrupt. */
  hasBeenActive: boolean
  /** A reason fired: an arrival was just toasted in this focused tab, or the
   *  tab came back into view after an arrival landed while it was hidden. */
  trigger: boolean
}

/** May the card show NOW? Every condition must hold. */
export function shouldAsk(i: AskInput): boolean {
  if (i.inShell) return false // the shell asks for itself (shouldAskShell, below)
  if (i.permission !== 'default') return false // granted, denied, insecure, unsupported
  if (!i.trigger) return false
  if (!i.hasBeenActive) return false
  if (i.now - i.loadedAt < MIN_LOADED_MS) return false
  const m = i.memory
  if (!m) return false
  if (m.granted) return false
  if (m.count >= PROMPT_MAX_DISMISSALS) return false
  if (m.nextAskAt) {
    const next = Date.parse(m.nextAskAt)
    if (Number.isFinite(next) && i.now < next) return false
  }
  return true
}

/** What a "Turn on" click came back with, as the card and the bell say it. */
export type TurnOnResult =
  | { kind: 'on' }
  | { kind: 'closed'; message: string }
  | { kind: 'blocked' }
  | { kind: 'unavailable'; message: string }

/** Read the permission a "Turn on" click ended in. A prompt closed with no
 *  answer leaves `default` — that is not a dismissal of OUR card, and the
 *  person is told they can try again. */
export function turnOnResult(after: PermissionState): TurnOnResult {
  switch (after) {
    case 'granted':
      return { kind: 'on' }
    case 'default':
      return { kind: 'closed', message: "The browser's prompt closed without an answer — Turn on to try again." }
    case 'denied':
      return { kind: 'blocked' }
    case 'insecure':
      return { kind: 'unavailable', message: 'Desktop alerts need HTTPS on this origin.' }
    default:
      return { kind: 'unavailable', message: "This browser doesn't support desktop alerts." }
  }
}

// ── The desktop shell's ask (macOS) ──────────────────────────────────────────

/** What the shell says the OS allows (desktop-shell.ts). Only macOS reports
 *  `not-determined`: Linux and Windows have no prompt, so the card below never
 *  shows there. */
export type ShellPermission = ShellNotificationStatus['permission']

/** This instance's memory of the card: `dismissed` (Not now) or `answered`
 *  (the OS prompt ran). Per instance: each one's page has its own storage. */
export const SHELL_ASK_KEY = 'talaria.desktopNotifyAsk'

export interface ShellAskInput {
  inShell: boolean
  /** The shell's answer; null: a shell that predates the command, or unread. */
  permission: ShellPermission | null | undefined
  /** The card was answered or put off here before. */
  remembered: boolean
  now: number
  loadedAt: number
  hasBeenActive: boolean
}

/** May the in-instance card show NOW? Every condition must hold. The card is
 *  ours, not the OS prompt: that still runs only inside its button's click
 *  (never at launch). It waits, like the browser's card, for a page the
 *  person has been on a while and touched. */
export function shouldAskShell(i: ShellAskInput): boolean {
  if (!i.inShell) return false
  if (i.permission !== 'not-determined') return false
  if (i.remembered) return false
  if (!i.hasBeenActive) return false
  return i.now - i.loadedAt >= MIN_LOADED_MS
}

/** What the card shows. `ask`: the invitation (busy while macOS asks, with a
 *  line when the last try did not settle it); `blocked`: the person said no
 *  in the OS prompt, and where to change that. */
export type ShellAskView =
  | { kind: 'hidden' }
  | { kind: 'ask'; os: string; busy: boolean; message: string | null }
  | { kind: 'blocked' }

export interface ShellAskEnv {
  inShell: () => boolean
  /** `shellNotificationStatus`. */
  status: () => Promise<ShellNotificationStatus | null>
  /** `shellRequestNotificationPermission`: the OS prompt. From a click only. */
  request: () => Promise<ShellNotificationStatus | null>
  memory: { get(): string | null; set(value: string): void }
  now: () => number
  loadedAt: number
  hasBeenActive: () => boolean
  onView: (v: ShellAskView) => void
}

export interface ShellAsk {
  /** Read the shell's permission and decide. `later`: not yet (too soon, or
   *  the page untouched) — call again on a timer or an input; `no`: nothing
   *  to ask (granted, denied, an older shell, answered before). */
  check(): Promise<'shown' | 'later' | 'no'>
  /** The card's button: the OS prompt, inside this click. */
  allow(): Promise<void>
  /** "Not now": remembered, the card does not come back to this instance. */
  notNow(): void
  /** Close the blocked note. */
  close(): void
  view(): ShellAskView
}

const osName = (s: ShellNotificationStatus | null) => (s?.backend === 'un' || !s ? 'macOS' : 'your system')

/** The card's logic over injected edges (the shell's two commands, storage,
 *  the clock), so a faked shell drives every path in a unit test. */
export function createShellAsk(env: ShellAskEnv): ShellAsk {
  let view: ShellAskView = { kind: 'hidden' }
  // Settled on this page: the card does not come back before a reload,
  // whatever the storage did with the write.
  let settled = false
  const set = (v: ShellAskView) => {
    view = v
    env.onView(v)
  }
  const remembered = () => {
    try {
      return settled || env.memory.get() !== null
    } catch {
      return settled
    }
  }
  const remember = (value: 'dismissed' | 'answered') => {
    settled = true
    try {
      env.memory.set(value)
    } catch {
      /* this page remembers it */
    }
  }
  const check = async (): Promise<'shown' | 'later' | 'no'> => {
    if (!env.inShell() || (remembered() && view.kind === 'hidden')) return 'no'
    if (view.kind === 'ask' && view.busy) return 'shown'
    let s: ShellNotificationStatus | null
    try {
      s = await env.status()
    } catch {
      return 'later'
    }
    if (s?.permission !== 'not-determined') {
      // Allowed or refused elsewhere (the launcher, Settings → Desktop, another
      // instance's card, System Settings): the question is answered.
      if (view.kind === 'ask') set({ kind: 'hidden' })
      return 'no'
    }
    if (view.kind !== 'hidden') return 'shown'
    const ask = shouldAskShell({
      inShell: true,
      permission: s.permission,
      remembered: remembered(),
      now: env.now(),
      loadedAt: env.loadedAt,
      hasBeenActive: env.hasBeenActive(),
    })
    if (!ask) return remembered() ? 'no' : 'later'
    set({ kind: 'ask', os: osName(s), busy: false, message: null })
    return 'shown'
  }
  const allow = async () => {
    if (view.kind !== 'ask' || view.busy) return
    const os = view.os
    set({ kind: 'ask', os, busy: true, message: null })
    let s: ShellNotificationStatus | null
    try {
      s = await env.request()
    } catch (e) {
      const msg = String(e instanceof Error ? e.message : e)
      set({
        kind: 'ask',
        os,
        busy: false,
        message: /rate limited/i.test(msg) ? 'Asked a moment ago. Try again in a few seconds.' : `The prompt did not open: ${msg}`,
      })
      return
    }
    switch (s?.permission) {
      case 'granted':
        remember('answered')
        set({ kind: 'hidden' })
        return
      case 'denied':
        remember('answered')
        set({ kind: 'blocked' })
        return
      case 'not-determined':
        set({ kind: 'ask', os, busy: false, message: 'No answer yet. Try again, or later from Settings.' })
        return
      case 'unavailable':
        set({
          kind: 'ask',
          os,
          busy: false,
          message: `Notifications are unavailable: ${(s.detail ?? 'no reason given').replace(/^unavailable:\s*/, '')}`,
        })
        return
      default:
        // A shell that has no such command: nothing this card can do.
        remember('answered')
        set({ kind: 'hidden' })
    }
  }
  return {
    check,
    allow,
    notNow: () => {
      remember('dismissed')
      set({ kind: 'hidden' })
    },
    close: () => set({ kind: 'hidden' }),
    view: () => view,
  }
}
