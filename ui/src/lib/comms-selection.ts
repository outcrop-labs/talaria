// THE CHANNEL OR THREAD YOU WERE LAST IN, remembered by Comms itself.
//
// Comms already puts its selection in the URL (`/comms/channel/<id>` or
// `/comms/agent/<model>[/<thread>]`) and derives everything from it, which is right: the
// view is copy-linkable and back/forward walks your reading order. What the URL
// cannot do is survive leaving the view — the nav rail sends you to `/comms`,
// which carries no selection, so the default-selection effect does exactly what
// it is written to do and lands you on the first channel. Coming back from a
// board therefore lost the agent thread you were in.
//
// WHY THIS LIVES IN THE VIEW AND NOT IN THE RAIL. The first attempt at this
// remembered the last href per nav section and had the rail link to it. It
// cannot be made reliable: the href has to be written from the router's
// location, and sv-router updates `route.pathname` and `searchParams` at
// different points inside one navigation, so the recorder can see a real
// pathname beside the next page's empty search and store `/comms` bare over the
// thread. Rewriting nav hrefs as you move also makes the rail itself feel
// unpredictable, which it did.
//
// A selection is not a URL and does not need the router at all. Comms derives
// `sel` from its own search params; this records that value and hands it back
// when Comms arrives with nothing selected. There is no moment in a navigation
// at which the two can disagree, because there is only one of them.
//
// SESSION-SCOPED, per tab: "where I was" is true of a sitting, not of a browser
// profile, and two tabs open on two channels should not fight over one key.

import { isUnder } from './route-tabs'
import { viewMemory } from './view-memory'

export type CommsSelection =
  | { t: 'channel'; id: string }
  | { t: 'agent'; model: string; conversationId: string | null }

function parse(raw: unknown): CommsSelection | null {
  if (!raw || typeof raw !== 'object') return null
  const v = raw as Record<string, unknown>
  if (v.t === 'channel' && typeof v.id === 'string' && v.id) return { t: 'channel', id: v.id }
  if (v.t === 'agent' && typeof v.model === 'string' && v.model) {
    return { t: 'agent', model: v.model, conversationId: typeof v.x === 'string' && v.x ? v.x : null }
  }
  return null
}

// `x` rather than `conversationId` in storage is the existing on-disk shape;
// keep it, or every open tab's memory is silently discarded on upgrade.
const memory = viewMemory<CommsSelection>('talaria:comms-selection', parse, (sel) =>
  sel.t === 'channel' ? { t: 'channel', id: sel.id } : { t: 'agent', model: sel.model, x: sel.conversationId },
)

export function readCommsSelection(): CommsSelection | null {
  return memory.read()
}

export function writeCommsSelection(sel: CommsSelection): void {
  memory.write(sel)
}

/**
 * The remembered selection, if it still refers to something that exists.
 *
 * VALIDATED AGAINST LIVE ROSTERS, because the alternative is landing someone in
 * a channel they were removed from or an agent that was retired — a worse
 * outcome than the first-channel default this replaces. A thread that has since
 * been deleted degrades to the agent with a fresh thread rather than throwing
 * the whole selection away: the agent is still the thing you were talking to.
 *
 * Pass `conversationIds: null` when that list has not loaded — the thread is
 * then taken on trust, which is correct, since discarding it for a list that
 * merely has not arrived yet is the same race in a new place.
 */
export function restorableSelection(
  saved: CommsSelection | null,
  rosters: { channelIds: string[]; agentModels: string[]; conversationIds: string[] | null },
): CommsSelection | null {
  if (!saved) return null
  if (saved.t === 'channel') return rosters.channelIds.includes(saved.id) ? saved : null
  if (!rosters.agentModels.includes(saved.model)) return null
  if (saved.conversationId && rosters.conversationIds && !rosters.conversationIds.includes(saved.conversationId)) {
    return { t: 'agent', model: saved.model, conversationId: null }
  }
  return saved
}

/**
 * Is this path inside Comms at all?
 *
 * THE REASON THIS IS A FUNCTION AND NOT AN INLINE CHECK. `route.pathname` flips
 * the instant a nav rail item is clicked, while the view that is leaving stays
 * mounted for a beat afterwards — so its effects run at least once against a URL
 * that already points elsewhere. Comms' default-selection effect navigates, and
 * an effect that navigates while answering a question about a page you are no
 * longer on drags you back to it.
 *
 * That was a live bug and it read as a broken nav rail: leaving Comms recomputed
 * the selection to null, the effect dutifully restored the remembered channel,
 * and the click was undone. It is a race against unmount, so it did not happen
 * every time — you clicked the same rail item two or three times before it took.
 */
export function isCommsPath(pathname: string): boolean {
  return isUnder(pathname, '/comms')
}

/**
 * The selection encoded in a Comms URL, or null if there isn't one — INCLUDING
 * when the path is not Comms' at all.
 *
 * The path is the selection and it is discriminated, because a bare one-segment
 * id would be ambiguous between a channel and an agent model until something
 * resolved it:
 *
 *   /comms/channel/<id>
 *   /comms/agent/<model>[/<thread>]
 *
 * The `/comms` base check is load-bearing rather than defensive: without it any
 * path whose second segment reads `channel` or `agent` parses as a Comms
 * selection, and the view is still mounted while the URL is someone else's.
 */
export function commsSelectionFromPath(pathname: string): CommsSelection | null {
  if (!isCommsPath(pathname)) return null
  const [, , kind, one, two] = pathname.split('/')
  if (kind === 'channel' && one) return { t: 'channel', id: decodeURIComponent(one) }
  if (kind === 'agent' && one) {
    return { t: 'agent', model: decodeURIComponent(one), conversationId: two ? decodeURIComponent(two) : null }
  }
  return null
}

/** Tests only: drop the remembered selection and the load latch. */
export function resetCommsSelection(): void {
  memory.reset()
}
