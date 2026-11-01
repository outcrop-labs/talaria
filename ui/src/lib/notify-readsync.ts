// READ ONE PLACE, CLEARED EVERYWHERE (I-16). Every read path publishes the
// read-sync event on the person's stream — `{type: "notifications", unread,
// unreadForYou, readIds?, all?, resync?}` — and every client of theirs takes
// down what it put up for those rows:
//
//   · `readIds` — close the OS banners whose tag is one of them (the service
//     worker registration's `getNotifications()`; the page and the worker
//     post into one tag namespace, keyed by row id), dismiss their in-app
//     toasts, and inside the desktop shell ask it to close its own
//     (`desktop_notify_close`, I-22). A catch-up summary whose rows are all
//     read goes too.
//   · `all` (a mark-all) — every Talaria banner and every notification toast.
//   · `resync` (more than 100 rows at once: no ids) — refetch, then close
//     every banner whose row is no longer unread.
//
// A counts-only event (no ids, no flag) closes nothing: it does not say
// which rows were read. Every tab of a browser runs this (the hub hands them
// all the frame); closing a banner that is already gone is a no-op.
//
// Framework-free over injected edges (notify-readsync.test.ts).

import type { NotificationsEventV2 } from './notify-wire'
import { SUMMARY_TAG } from './notify-wire'

const ROW_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** The most tags the shell's `desktop_notify_close` takes in one call (it
 *  drops the rest): longer lists go in chunks, the summary tag first. */
export const SHELL_CLOSE_MAX = 100

/** One banner the registration still shows. */
export interface ShownBanner {
  tag: string
  close(): void
}

export interface ReadSyncDeps {
  /** The banners this origin has up (the registration's), [] without one. */
  shown: () => Promise<ShownBanner[]>
  /** Dismiss the in-app toasts for these keys, or all of them. */
  dismiss: (keys: string[] | 'all') => void
  /** The toast keys on screen. */
  toastKeys: () => string[]
  /** Inside the desktop shell: close its banners for these tags. */
  shellClose?: (tags: string[]) => Promise<boolean>
  /** The tags this browser bannered recently (the bannered store) — what a
   *  shell's `all`/`resync` close asks about, since the page cannot list the
   *  shell's banners. */
  recentTags?: () => Promise<string[]>
  /** The rows the summary banner on screen stands for (empty: none up). */
  summaryIds?: () => string[]
  /** The summary banner is gone (closed here). */
  summaryClosed?: () => void
  /** Resync: refetch, and answer which rows are read — `unread` holds every
   *  unread id when `complete` (the list is all of them), else only the ones
   *  the read saw; `read` the ids it saw read. */
  refetch?: () => Promise<{ unread: Set<string>; read: Set<string>; complete: boolean }>
}

export interface ReadSyncResult {
  /** Banner tags closed on the registration. */
  closed: string[]
  /** Tags asked of the shell. */
  shell: string[]
  /** Toasts dismissed (keys, or 'all'). */
  dismissed: string[] | 'all'
}

const isRowTag = (tag: string) => ROW_ID.test(tag)

/** Apply one read-sync event. */
export async function applyReadSync(ev: NotificationsEventV2, deps: ReadSyncDeps): Promise<ReadSyncResult> {
  const out: ReadSyncResult = { closed: [], shell: [], dismissed: [] }
  if (ev.all) {
    const shown = await safe(deps.shown, [])
    for (const b of shown) {
      b.close()
      out.closed.push(b.tag)
    }
    deps.dismiss('all')
    out.dismissed = 'all'
    if (deps.summaryIds?.().length) deps.summaryClosed?.()
    if (deps.shellClose) {
      const tags = uniq([SUMMARY_TAG, ...(await safe(deps.recentTags ?? (async () => []), []))])
      out.shell = tags
      await closeInShell(deps.shellClose, tags)
    }
    return out
  }
  if (Array.isArray(ev.readIds) && ev.readIds.length > 0) {
    const read = new Set(ev.readIds)
    const summary = deps.summaryIds?.() ?? []
    const summaryDone = summary.length > 0 && summary.every((id) => read.has(id))
    const shown = await safe(deps.shown, [])
    for (const b of shown) {
      if (read.has(b.tag) || (summaryDone && b.tag === SUMMARY_TAG)) {
        b.close()
        out.closed.push(b.tag)
      }
    }
    const keys = deps.toastKeys().filter((k) => read.has(k) || (summaryDone && k === SUMMARY_TAG))
    if (keys.length) deps.dismiss(keys)
    out.dismissed = keys
    if (summaryDone) deps.summaryClosed?.()
    if (deps.shellClose) {
      const tags = uniq([...(summaryDone ? [SUMMARY_TAG] : []), ...ev.readIds])
      out.shell = tags
      await closeInShell(deps.shellClose, tags)
    }
    return out
  }
  if (ev.resync && deps.refetch) {
    const fresh = await deps.refetch().catch(() => null)
    if (!fresh) return out
    const gone = (tag: string) => isRowTag(tag) && (fresh.read.has(tag) || (fresh.complete && !fresh.unread.has(tag)))
    const shown = await safe(deps.shown, [])
    for (const b of shown) {
      if (gone(b.tag)) {
        b.close()
        out.closed.push(b.tag)
      }
    }
    const keys = deps.toastKeys().filter(gone)
    if (keys.length) deps.dismiss(keys)
    out.dismissed = keys
    if (deps.shellClose) {
      const tags = (await safe(deps.recentTags ?? (async () => []), [])).filter(gone)
      out.shell = tags
      await closeInShell(deps.shellClose, tags)
    }
  }
  return out
}

async function safe<T>(fn: () => Promise<T>, fallback: T): Promise<T> {
  try {
    return await fn()
  } catch {
    return fallback
  }
}

const uniq = (xs: string[]) => [...new Set(xs)]

/** Ask the shell to close these tags, `SHELL_CLOSE_MAX` per call. */
async function closeInShell(close: (tags: string[]) => Promise<boolean>, tags: string[]): Promise<void> {
  for (let i = 0; i < tags.length; i += SHELL_CLOSE_MAX) {
    await close(tags.slice(i, i + SHELL_CLOSE_MAX)).catch(() => false)
  }
}

/** The browser's banners, from the service worker registration. */
export async function registrationBanners(): Promise<ShownBanner[]> {
  if (typeof navigator === 'undefined' || !('serviceWorker' in navigator)) return []
  const reg = await navigator.serviceWorker.getRegistration()
  if (!reg) return []
  return (await reg.getNotifications()).map((n) => ({ tag: n.tag, close: () => n.close() }))
}
