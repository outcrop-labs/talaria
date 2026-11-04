// The notification runtime's live status, for the status panel (C16): is the
// person's event stream open, which of their clients is the one they are on
// (I-17), and what this tab's role is among the tabs (the leader that posts
// banners, a follower, or alone). A STUB the contract lands so the two wave-2
// streams that meet here never edit each other's files: W2-R (the web
// runtime) calls the setters; W2-C (the notification center) renders the
// getter. Nothing in phase 0 calls either.
//
// Module-level $state, the desktop-settings shape: one store per tab.

export type StreamState = 'connecting' | 'open' | 'reconnecting' | 'closed'

/** The presence winner as the last `presence` frame (or GET) told it. */
export interface PresenceStatus {
  looking: string | null
  active: string | null
  clients: number
  /** This tab's own client id, so the panel can say "this tab". */
  self: string | null
}

export type HubRole = 'leader' | 'follower' | 'solo' | 'unknown'

export interface NotifyStatus {
  stream: { state: StreamState; since: number | null; lastEventAt: number | null }
  presence: PresenceStatus
  hubRole: HubRole
}

const initial = (): NotifyStatus => ({
  stream: { state: 'closed', since: null, lastEventAt: null },
  presence: { looking: null, active: null, clients: 0, self: null },
  hubRole: 'unknown',
})

let status = $state<NotifyStatus>(initial())

/** The current status (reactive inside a component). */
export function notifyStatus(): NotifyStatus {
  return status
}

/** The event stream moved: `at` is when (ms), defaulting to now. An event
 *  arriving on an open stream passes `lastEventAt` alone. */
export function setStreamState(state: StreamState, at: number = Date.now()): void {
  status.stream = {
    state,
    since: status.stream.state === state ? status.stream.since : at,
    lastEventAt: status.stream.lastEventAt,
  }
}

/** A frame arrived on the stream. */
export function noteStreamEvent(at: number = Date.now()): void {
  status.stream = { ...status.stream, lastEventAt: at }
}

/** The presence winner changed (a `presence` frame, or a GET). */
export function setPresence(p: Partial<PresenceStatus>): void {
  status.presence = { ...status.presence, ...p }
}

/** This tab's role among the person's tabs changed. */
export function setHubRole(role: HubRole): void {
  status.hubRole = role
}

/** Back to the start (sign-out, tests). */
export function resetNotifyStatus(): void {
  status = initial()
}
