// WHERE YOU WERE, PER VIEW.
//
// Every surface here already keeps its state in the URL — Comms is `?c=<channel>`
// or `?a=<agent>&x=<thread>`, a board is `/boards/<id>?view=list&group=priority`.
// That was a deliberate convention and it works: paste the link, get the state.
// The rail then threw it away on every trip, because a nav item is a bare path.
// So leaving Comms for a board and coming back did not "lose" your channel —
// it navigated to `/comms`, which has no channel, and the view's default-select
// effect did what it is supposed to and picked the first one.
//
// This remembers the last href per section so the rail can send you back where
// you were. Nothing about the views changes; they still read the URL.
//
// SESSION-SCOPED, NOT DURABLE. "Where I was" is true of a sitting, not of a
// browser profile — coming back tomorrow and landing in whatever channel you
// happened to close is worse than landing on the default. sessionStorage is
// also per-tab, so two tabs open on two different channels do not fight over
// one key. Durable *preferences* (a board's layout) are a different thing and
// live in localStorage; see Board.svelte.

const KEY = 'talaria:view-memory'
export const VIEW_MEMORY_EVENT = 'talaria:view-memory'

/** In-memory mirror, and the whole store when sessionStorage is unavailable
 *  (private mode, SSR). Read through `snapshot()`, never directly. */
let fallback: Record<string, string> = {}
let loaded = false

function read(): Record<string, string> {
  if (loaded) return fallback
  loaded = true
  try {
    const raw = window.sessionStorage.getItem(KEY)
    const parsed: unknown = raw ? JSON.parse(raw) : {}
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      fallback = Object.fromEntries(
        Object.entries(parsed as Record<string, unknown>).filter(([, v]) => typeof v === 'string'),
      ) as Record<string, string>
    }
  } catch {
    /* unreadable or malformed: an empty memory is a correct memory */
  }
  return fallback
}

function write(next: Record<string, string>): void {
  fallback = next
  try {
    window.sessionStorage.setItem(KEY, JSON.stringify(next))
  } catch {
    /* private mode: the in-memory mirror still serves this tab */
  }
  try {
    window.dispatchEvent(new Event(VIEW_MEMORY_EVENT))
  } catch {
    /* no window: nothing is listening anyway */
  }
}

/**
 * The nav section a path belongs to — the unit that gets remembered.
 *
 * An app's two surfaces are separate sections: `/x/<slug>` (Work) is a path
 * prefix of `/x/<slug>/manage` (Manage), and collapsing them would make the
 * Manage link navigate to the work surface. NavRail's `exactFor` draws the
 * same distinction for the same reason.
 */
export function sectionKey(pathname: string): string {
  const parts = pathname.split('/').filter(Boolean)
  if (parts.length === 0) return '/'
  if (parts[0] === 'x') return parts[2] === 'manage' ? `/x/${parts[1]}/manage` : `/x/${parts[1] ?? ''}`
  return `/${parts[0]}`
}

/**
 * Record a location as its section's last position.
 *
 * ONE ARGUMENT, ON PURPOSE. This took a `(pathname, search)` pair, and the
 * caller sourced them from two different reactive mirrors: `route.pathname`
 * (sv-router's `location` state) and `searchParams.toString()` (a separate
 * SvelteURLSearchParams). `onNavigate` updates those a dozen lines apart, so
 * a flush landing between them recorded a real pathname with the *next* page's
 * empty search — `/comms` bare, over the agent thread you were in. A pair that
 * can disagree should not be a signature; an href cannot disagree with itself.
 *
 * STRIPPED IS NOT CHOSEN. A record for the very same pathname with no query
 * never replaces one that has a query. Across these surfaces a bare
 * `/comms` is not a place anybody sits — the view replaces it with `?c=` or
 * `?a=` the moment it can — so it only ever shows up mid-flight, whether from
 * a torn-down view's default-select or a mirror that had not caught up. A
 * DIFFERENT pathname is a real move and always wins, so the boards index still
 * overwrites a board.
 */
export function rememberView(href: string): void {
  const path = href.split('?')[0] || '/'
  const key = sectionKey(path)
  const current = read()
  if (current[key] === href) return
  const previous = current[key]
  if (previous && !href.includes('?') && previous.split('?')[0] === path) return
  write({ ...current, [key]: href })
}

/** Drop a section's memory — call this when the place it points at is gone,
 *  or the rail keeps walking back into a deleted board forever. */
export function forgetView(pathname: string): void {
  const key = sectionKey(pathname)
  const current = read()
  if (!(key in current)) return
  const next = { ...current }
  delete next[key]
  write(next)
}

/**
 * The href a nav item should actually point at.
 *
 * Falls back to the bare path whenever the memory does not obviously belong to
 * this item. The stored value is checked against the item it is about to be
 * used for — sessionStorage is writable by anything running on this origin,
 * and a nav link is not the place to find out.
 */
export function hrefFor(to: string): string {
  const remembered = read()[sectionKey(to)]
  if (!remembered || !remembered.startsWith('/') || remembered.startsWith('//')) return to
  if (sectionKey(remembered.split('?')[0]!) !== sectionKey(to)) return to
  return remembered
}

export function subscribeViewMemory(onChange: () => void): () => void {
  window.addEventListener(VIEW_MEMORY_EVENT, onChange)
  return () => window.removeEventListener(VIEW_MEMORY_EVENT, onChange)
}

/** Tests only: drop every remembered position and the load latch. */
export function resetViewMemory(): void {
  fallback = {}
  loaded = false
}
