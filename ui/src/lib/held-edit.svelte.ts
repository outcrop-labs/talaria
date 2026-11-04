// A REFUSED BUFFER IS KEPT ON THE DEVICE until the person decides.
//
// When a save is refused (lib/save-base: an agent or another person saved
// the document under the editor), what the person types afterwards is not
// sent anywhere — the server would refuse it again. Before this module it
// lived only in the live editor, so any way out other than the Read toggle
// threw it away without a word: another doc in the tree, a backlink, Back,
// closing the tab, or the Work view swapping the file pane for its comments.
// Now, while a save is refused:
//
//   • every refused attempt (each autosave pause, each blur) is written to
//     localStorage under the document AND the viewer, and once more from the
//     live editor when it unmounts or the page hides;
//   • the router asks before an in-app navigation leaves the editor, and the
//     browser asks before the tab closes (sv-router's blockNavigation);
//   • the next time this person opens the document, what they left is
//     offered back — Keep mine saves it over the newer version, Discard
//     drops it. An offer whose text the document already holds is stale and
//     is dropped without asking.
//
// The stash is cleared when the refusal is resolved here (Keep mine, a
// restore, Discard). localStorage can be absent or refuse access (private
// mode, blocked site data): lib/persist absorbs that, and then the editor
// behaves as before — the prompts still stand between the person and the
// loss, the device simply cannot remember.

import { blockNavigation } from 'sv-router'
import { readStored, writeStored, writeText } from '@/lib/persist'
import type { HistoryKind } from '@/lib/revisions.svelte'

export interface HeldEdit {
  /** Present only when the person renamed the document too. */
  title?: string
  body: string
  /** When it was kept (ISO). */
  at: string
}

/** Per document, per viewer: two people sharing a browser share nothing. */
export const heldKey = (kind: HistoryKind, id: string, userId: string) => `talaria:refused:${kind}:${id}:${userId}`

export function parseHeld(v: unknown): HeldEdit | null {
  if (!v || typeof v !== 'object') return null
  const o = v as Record<string, unknown>
  if (typeof o.body !== 'string' || typeof o.at !== 'string') return null
  if (o.title !== undefined && typeof o.title !== 'string') return null
  return o.title === undefined ? { body: o.body, at: o.at } : { title: o.title, body: o.body, at: o.at }
}

export const readHeld = (key: string): HeldEdit | null => readStored(key, parseHeld, null)

export function writeHeld(key: string, edit: HeldEdit | null): void {
  if (edit === null) writeText(key, null)
  else writeStored(key, edit)
}

interface Refusable {
  readonly conflict: boolean
  readonly held: { title?: string; body?: string } | null
  readonly busy: boolean
}

/** Wire one editor's refused buffer to the device. Call during component
 *  init. `live` reads the mounted surface's buffer (undefined when nothing is
 *  mounted, or it cannot be read); `current` is the document as loaded;
 *  `leave` is the in-app "leave anyway?" ask. */
export function holdRefusedEdits(opts: {
  key: () => string | null
  saver: Refusable
  editing: () => boolean
  live: () => { title?: string; body: string } | undefined
  current: () => { body: string } | null | undefined
  leave: () => Promise<boolean>
}) {
  let offered = $state<HeldEdit | null>(null)
  let looked = false
  let wrote = false
  const refused = () => opts.saver.conflict && opts.editing()

  const keep = (edit: { title?: string; body?: string } | undefined) => {
    const key = opts.key()
    if (!key || edit?.body === undefined) return
    writeHeld(key, { ...(edit.title !== undefined ? { title: edit.title } : {}), body: edit.body, at: new Date().toISOString() })
    wrote = true
  }
  const liveOrHeld = () => {
    try {
      return opts.live() ?? opts.saver.held ?? undefined
    } catch {
      return opts.saver.held ?? undefined
    }
  }

  // The offer: read once, when the viewer and the document are both known.
  $effect(() => {
    const key = opts.key()
    const doc = opts.current()
    if (looked || !key || !doc) return
    looked = true
    const kept = readHeld(key)
    if (kept && kept.body !== doc.body) offered = kept
    else if (kept) writeHeld(key, null)
  })

  // Every refused attempt is kept; a fresh refusal supersedes an old offer.
  $effect(() => {
    if (!refused() || !opts.saver.held) return
    keep(opts.saver.held)
    offered = null
  })

  // Resolved here (keep mine, a restore, discard): nothing left to keep.
  $effect(() => {
    if (opts.saver.conflict || !wrote) return
    wrote = false
    const key = opts.key()
    if (key && offered === null) writeHeld(key, null)
  })

  // The last words: the live buffer when the editor goes or the page hides.
  $effect(() => {
    const onHide = () => {
      if (refused()) keep(liveOrHeld())
    }
    const win = typeof window === 'undefined' ? null : window
    win?.addEventListener('pagehide', onHide)
    return () => {
      win?.removeEventListener('pagehide', onHide)
      onHide()
    }
  })

  // Ask before leaving: the router for in-app moves, the browser for the tab.
  $effect(() =>
    blockNavigation({
      beforeUnload: () => !(refused() || opts.saver.busy),
      onNavigate: async () => {
        if (!refused()) return true
        keep(liveOrHeld())
        return await opts.leave()
      },
    }),
  )

  /** Decided (kept or discarded): forget it on this device. */
  const settle = () => {
    offered = null
    wrote = false
    const key = opts.key()
    if (key) writeHeld(key, null)
  }

  return {
    /** What this person left here on an earlier visit, not yet decided. */
    get offered() {
      return offered
    },
    settle,
    /** Keep mine on the offer: `replace` saves it over the newer version
     *  (resolving whether it landed); once it has, the offer is settled. */
    async keepWith(replace: (edit: { title?: string; body: string }) => Promise<boolean>) {
      const kept = offered
      if (!kept) return
      if (await replace(kept.title !== undefined ? { title: kept.title, body: kept.body } : { body: kept.body })) settle()
    },
  }
}
