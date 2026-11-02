// The app's toast stack: transient, one per event, click to go there.
//
// Anywhere in the client can `pushToast({ title, body, href })`; the single
// <Toasts /> host in AppLayout renders them. Toasts are the IN-APP half of
// "tell me now" — the browser-notification half (for when Talaria is not what
// the person is looking at) lives in browser-notify.ts and is fired by the
// same call site, not by this store: a toast is always ours to show, an OS
// notification never is.
export type ToastTone = 'info' | 'success' | 'danger'
import { errorMessage } from '@/lib/fetch-json'

export interface ToastSpec {
  title: string
  body?: string
  /** Route to open on click. */
  href?: string
  tone?: ToastTone
  /** What a click does INSTEAD of opening `href` — a notification toast's
   *  click routes and marks its row read (lib/notify-open.ts). */
  onOpen?: () => void
  /** What the toast is ABOUT — a notification row's id (or the catch-up
   *  summary's tag): a read anywhere dismisses it (I-16, notify-readsync.ts),
   *  and a second toast for the same key replaces the first. */
  key?: string
}

export interface ToastItem {
  id: number
  title: string
  body?: string
  href?: string
  tone: ToastTone
  onOpen?: () => void
  key?: string
}

// Long enough to read two lines; short enough that a stack of these never
// walls off a corner of the screen for long. A click dismisses instantly.
const TOAST_TTL_MS = 6_500

// The stack is a heads-up, not a log. Past four, the oldest goes — if four
// arrived while the person was away, the inbox is the honest catch-up.
const MAX_STACK = 4

const store = $state<{ items: ToastItem[] }>({ items: [] })
let nextId = 1
const timers = new Map<number, ReturnType<typeof setTimeout>>()

/** A danger toast carrying an error's message — the shape 93 call sites wrote
 *  out by hand (`pushToast({ title, body: errorMessage(e), tone: 'danger' })`).
 *  The pairing is the point: an error toast whose body is not the server's own
 *  sentence is a toast that stopped the person from reading it. */
export function toastError(title: string, e: unknown): void {
  pushToast({ title, body: errorMessage(e), tone: 'danger' })
}

export function pushToast(spec: ToastSpec): void {
  const item: ToastItem = {
    id: nextId++,
    title: spec.title,
    body: spec.body,
    href: spec.href,
    tone: spec.tone ?? 'info',
    onOpen: spec.onOpen,
    ...(spec.key ? { key: spec.key } : {}),
  }
  // A re-alert of the same row replaces its toast instead of stacking.
  if (spec.key) dismissToastsByKey([spec.key])
  store.items.push(item)
  while (store.items.length > MAX_STACK && store.items[0]) dismissToast(store.items[0].id)
  timers.set(item.id, setTimeout(() => dismissToast(item.id), TOAST_TTL_MS))
}

export function dismissToast(id: number): void {
  const i = store.items.findIndex((t) => t.id === id)
  if (i === -1) return
  store.items.splice(i, 1)
  const timer = timers.get(id)
  if (timer) {
    clearTimeout(timer)
    timers.delete(id)
  }
}

/** Dismiss every toast about these keys (rows read elsewhere). Answers how
 *  many went. */
export function dismissToastsByKey(keys: Iterable<string>): number {
  const set = new Set(keys)
  if (set.size === 0) return 0
  const ids = store.items.filter((t) => t.key !== undefined && set.has(t.key)).map((t) => t.id)
  for (const id of ids) dismissToast(id)
  return ids.length
}

/** Dismiss every keyed toast — every notification toast (a mark-all). */
export function dismissKeyedToasts(): number {
  const ids = store.items.filter((t) => t.key !== undefined).map((t) => t.id)
  for (const id of ids) dismissToast(id)
  return ids.length
}

/** The keys of the toasts on screen (read-sync's resync pass). */
export function toastKeys(): string[] {
  return store.items.flatMap((t) => (t.key === undefined ? [] : [t.key]))
}

/** Read API for the host component. */
export function toastList(): readonly ToastItem[] {
  return store.items
}
