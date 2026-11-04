// A document's version history (/api/history), read once for the whole page:
// the Version history rail lists it, and a READ pane polls it to follow the
// server. One query key per item, so the rail and the follower share a fetch
// — a revision the follower saw land is already in the open rail.

import { createQuery, type QueryClient } from '@tanstack/svelte-query'
import { getList } from '@/lib/fetch-json'

export interface Revision {
  id: string
  /** Who saved it, as the save route stamps it: a person's email (or name),
   *  or an agent's id for a toolkit save. */
  createdBy: string | null
  createdAt: string
  /** Character length of the snapshot. */
  size: number
}

export type HistoryKind = 'kb-doc' | 'kb-space' | 'artifact'

/** The same key shape InternalEditor uses (`['history', params]`). */
export const historyKey = (kind: HistoryKind, id: string) => ['history', { kind, id }] as const

/** How often a read pane asks whether the document moved. */
export const FOLLOW_INTERVAL_MS = 4_000

/** The newest-first page /api/history answers (`list_history`'s `limit 50`
 *  in talaria-internal-history). A list this long may have older versions
 *  behind it, so its last row has no listed predecessor to diff against. */
export const HISTORY_PAGE = 50

export function useRevisions(
  kind: () => HistoryKind,
  id: () => string,
  opts: { poll?: () => number | false; client?: () => QueryClient } = {},
) {
  return createQuery(
    () => ({
      queryKey: historyKey(kind(), id()),
      enabled: !!id(),
      refetchInterval: opts.poll?.() ?? false,
      queryFn: () =>
        getList<Revision>(`/api/history?kind=${kind()}&id=${encodeURIComponent(id())}`, 'revisions'),
    }),
    opts.client,
  )
}

/** The read pane's decision, pure: given the newest revision id it last saw
 *  (undefined before the first answer), did the document move? The FIRST
 *  answer is only a baseline — opening a document, or one an agent just
 *  created, is not news. */
export function newestMoved(seen: string | null | undefined, revisions: Revision[]): boolean {
  if (seen === undefined) return false
  return (revisions[0]?.id ?? null) !== seen
}

/** READ PANES FOLLOW THE SERVER. A read pane holds no buffer of its own, so
 *  when a revision lands — an agent's `edit_kb_doc` / `update_document`, or
 *  another person's save — it refreshes itself (`onMoved`, which invalidates
 *  the document's own query). No UI: the text simply changes; who changed it
 *  and how is the Version history rail's to show. Polls only while `active`
 *  (read mode); an edit pane never follows — its buffer is the person's, and
 *  the save route's base check is what keeps an agent's edit from being
 *  overwritten by it. A move seen while inactive (the rail refetching during
 *  an edit) is held, and the pane refreshes once it is a read pane again. */
export function followRevisions(
  kind: () => HistoryKind,
  id: () => string,
  opts: { active: () => boolean; onMoved: () => void; intervalMs?: number; client?: () => QueryClient },
) {
  const query = useRevisions(kind, id, {
    poll: () => (opts.active() ? (opts.intervalMs ?? FOLLOW_INTERVAL_MS) : false),
    client: opts.client,
  })
  let seen: string | null | undefined = undefined
  let pending = false
  $effect(() => {
    const revisions = query.data
    const active = opts.active()
    if (revisions) {
      if (newestMoved(seen, revisions)) pending = true
      seen = revisions[0]?.id ?? null
    }
    if (pending && active) {
      pending = false
      opts.onMoved()
    }
  })
  return query
}
