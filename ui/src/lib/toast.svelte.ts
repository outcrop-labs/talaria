// The app's toast stack: transient, one per event, click to go there.
//
// Anywhere in the client can `pushToast({ title, body, href })`; the single
// <Toasts /> host in AppLayout renders them. Toasts are the IN-APP half of
// "tell me now" — the browser-notification half (for when Talaria is not what
// the person is looking at) lives in browser-notify.ts and is fired by the
// same call site, not by this store: a toast is always ours to show, an OS
// notification never is.
export type ToastTone = 'info' | 'success' | 'danger'

export interface ToastSpec {
  title: string
  body?: string
  /** Route to open on click. */
  href?: string
  tone?: ToastTone
}

export interface ToastItem {
  id: number
  title: string
  body?: string
  href?: string
  tone: ToastTone
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

export function pushToast(spec: ToastSpec): void {
  const item: ToastItem = { id: nextId++, title: spec.title, body: spec.body, href: spec.href, tone: spec.tone ?? 'info' }
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

/** Read API for the host component. */
export function toastList(): readonly ToastItem[] {
  return store.items
}
