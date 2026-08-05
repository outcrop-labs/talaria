// Shared pieces of the Knowledge page (Knowledge.svelte + its editors/panels).
// Runes module: useDocLive carries a $effect heartbeat.
import { createQuery } from '@tanstack/svelte-query'
import { cn } from '@/lib/cn'
import { getJson, getList } from '@/lib/fetch-json'
import { useSession } from '@/lib/session'
import { searchKb } from '@/lib/kb'
import type { DocSearchFn } from '@/components/ui/rich-editor'

export interface DocPresence {
  userId: string
  name: string
  mode: 'view' | 'edit'
}

/** The doc's multiplayer heartbeat: announce presence (view/edit) while
 *  mounted, poll who else is here. Call during component init; `mode` is a
 *  getter so the beat follows the read/edit toggle. */
export function useDocLive(docId: string, mode: () => 'read' | 'edit') {
  $effect(() => {
    const beat = mode() === 'edit' ? 'edit' : 'view'
    const ping = () =>
      fetch(`/api/kb/docs/${docId}/live`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ mode: beat }),
      }).catch(() => {})
    void ping()
    const t = setInterval(() => void ping(), 25_000)
    return () => clearInterval(t)
  })
  return createQuery(() => ({
    queryKey: ['kb-live', docId],
    // "You're alone in this doc" must come from a real answer: a failed poll
    // that resolved to `[]` would drop the other editors' avatars — and the
    // co-editing warning that goes with them — mid-session.
    queryFn: (): Promise<DocPresence[]> => getList<DocPresence>(`/api/kb/docs/${docId}/live`, 'active'),
    refetchInterval: 15_000,
  }))
}

export interface KbComment {
  id: string
  docId: string
  parentId: string | null
  authorUserId: string | null
  author: string
  quote: string | null
  content: string
  resolved: boolean
  createdAt: string
}

export function useDocComments(docId: string) {
  return createQuery(() => ({
    queryKey: ['kb-comments', docId],
    // A failed 20s poll used to overwrite a loaded thread with an empty one —
    // the comment marks vanished out of the prose. Rejecting keeps the last
    // good thread on screen instead.
    queryFn: (): Promise<KbComment[]> => getList<KbComment>(`/api/kb/docs/${docId}/comments`, 'comments'),
    refetchInterval: 20_000,
  }))
}

/** True when the signed-in user owns this doc/space (only owners can re-share).
 *  Returns a `{ current }` box (the React hook re-ran per render; this stays
 *  reactive through the getter). */
export function useIsOwner(item: () => { ownerUserId: string | null; createdBy: string | null } | null | undefined): {
  readonly current: boolean
} {
  const session = useSession()
  return {
    get current() {
      const it = item()
      const me = session.data
      if (!it || !me) return false
      return it.ownerUserId ? it.ownerUserId === me.id : it.createdBy === (me.email ?? me.name)
    },
  }
}

// Shared cross-reference search for the editor's link button: knowledge docs
// AND artifacts — an artifact picked here embeds INLINE as a link at the
// caret (the bottom attachments strip stays for formal doc-artifact ties).
export const docSearch: DocSearchFn = async (q) => {
  if (!q.trim()) return []
  const [hits, artifacts] = await Promise.all([
    searchKb(q),
    // `r.ok ? r.json() : { artifacts: [] }` resolved a 500 into "no artifacts
    // match that" inside a link picker — the writer concludes the document
    // they are looking for does not exist and goes and makes another one.
    // getJson throws; the catch below still degrades to KB-only results rather
    // than failing the whole search, but only AFTER a real failure.
    getJson<{ artifacts?: Array<{ id: string; title: string; kind: string }> }>('/api/artifacts')
      .then((d) => (d.artifacts ?? []).filter((a) => a.title.toLowerCase().includes(q.trim().toLowerCase())).slice(0, 6))
      .catch(() => [] as Array<{ id: string; title: string; kind: string }>),
  ])
  return [
    ...hits
      .filter((h) => h.kind === 'doc')
      .slice(0, 6)
      .map((h) => ({ id: h.id, title: h.title || 'Untitled', icon: h.icon, href: `/knowledge?d=${h.id}` })),
    ...artifacts.map((a) => ({ id: a.id, title: a.title || 'Untitled', icon: '💎', href: `/artifacts?a=${a.id}` })),
  ]
}

// ── Editor helpers ──────────────────────────────────────────────────────────
export interface Heading {
  level: number
  text: string
}
export function parseHeadings(md: string): Heading[] {
  const out: Heading[] = []
  let inFence = false
  for (const line of md.split('\n')) {
    if (/^\s*```/.test(line)) inFence = !inFence
    if (inFence) continue
    const m = /^(#{1,3})\s+(.*)$/.exec(line)
    if (m) out.push({ level: m[1]!.length, text: m[2]!.replace(/[*_`]/g, '').trim() })
  }
  return out
}

// Editor shell: normal fill vs a fullscreen takeover — same column layout.
export const editorShell = (fullscreen: boolean) =>
  cn('flex min-h-0 flex-col', fullscreen ? 'fixed inset-0 z-50 bg-surface' : 'h-full')

// ── Page skeleton widths ────────────────────────────────────────────────────
// Matches the doc/space editor layout (breadcrumb, toolbar with icon + title +
// buttons, centered prose column) so the swap to real content doesn't jump.
export const PROSE_WIDTHS = ['100%', '92%', '97%', '86%', '95%', '73%', '100%', '90%', '96%', '88%', '94%', '58%']
