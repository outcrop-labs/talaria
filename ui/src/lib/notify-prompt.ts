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

import type { PermissionState } from '@/lib/browser-notify'

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
  if (i.inShell) return false // the shell asks for itself (Settings → Desktop)
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
