import { createFileRoute } from '@tanstack/react-router'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import {
  Plus, FileText, Bot, Globe, Lock, Users, Star, Trash2, History,
  ChevronRight, Search, Link2, ListTree, Maximize2, Minimize2, MoreHorizontal, Paperclip,
  MessageSquareText, Sparkles, CheckCircle2, CornerDownRight, Pencil,
  type LucideIcon,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { IconButton } from '@/components/ui/icon-button'
import { CloseButton } from '@/components/ui/close-button'
import { Segmented } from '@/components/ui/segmented'
import { Chip } from '@/components/ui/chip'
import { confirm } from '@/components/ui/confirm'
import { Input } from '@/components/ui/input'
import { inlineEditKeys } from '@/components/ui/control'
import { Modal } from '@/components/ui/modal'
import { Markdown } from '@/components/ui/markdown'
import { EmptyState } from '@/components/ui/empty-state'
import { listQuery, QueryError } from '@/components/ui/query-state'
import { EmojiPicker } from '@/components/ui/emoji-picker'
import { RichEditor, type RichEditorHandle, type DocSearchFn } from '@/components/ui/rich-editor'
import { useContextMenu, copyAppLink, DropdownMenu, type ContextMenuEntry } from '@/components/ui/context-menu'
import { PermissionsModal } from '@/components/kb/permissions-modal'
import { Combobox } from '@/components/ui/combobox'
import { useArtifacts, useTargetArtifacts, attachArtifact, detachArtifact } from '@/lib/artifacts'
import { cn } from '@/lib/cn'
import { getJson, getList } from '@/lib/fetch-json'
import { relativeTime } from '@/lib/fleet'
import { useSession } from '@/lib/session'
import { Avatar } from '@/components/ui/avatar'
import { Textarea } from '@/components/ui/textarea'
import { streamMuse } from '@/lib/muse'
import { BrainRoutingSelect } from '@/components/kb/brain-select'
import {
  createDoc, createSpace, deleteDoc, deleteSpace, moveDoc, saveDoc, searchKb, updateSpace, useBacklinks,
  useDoc, useDocs, useSpace, useSpaces,
  type KbDocMeta, type KbSearchHit, type KbSpace,
} from '@/lib/kb'

interface DocPresence {
  userId: string
  name: string
  mode: 'view' | 'edit'
}

/** The doc's multiplayer heartbeat: announce presence (view/edit) while
 *  mounted, poll who else is here. */
function useDocLive(docId: string, mode: 'read' | 'edit') {
  const beat = mode === 'edit' ? 'edit' : 'view'
  useEffect(() => {
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
  }, [docId, beat])
  return useQuery({
    queryKey: ['kb-live', docId],
    // "You're alone in this doc" must come from a real answer: a failed poll
    // that resolved to `[]` would drop the other editors' avatars — and the
    // co-editing warning that goes with them — mid-session.
    queryFn: (): Promise<DocPresence[]> => getList<DocPresence>(`/api/kb/docs/${docId}/live`, 'active'),
    refetchInterval: 15_000,
  })
}

interface KbComment {
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

function useDocComments(docId: string) {
  return useQuery({
    queryKey: ['kb-comments', docId],
    // A failed 20s poll used to overwrite a loaded thread with an empty one —
    // the comment marks vanished out of the prose. Rejecting keeps the last
    // good thread on screen instead.
    queryFn: (): Promise<KbComment[]> => getList<KbComment>(`/api/kb/docs/${docId}/comments`, 'comments'),
    refetchInterval: 20_000,
  })
}

// True when the signed-in user owns this doc/space (only owners can re-share).
function useIsOwner(item: { ownerUserId: string | null; createdBy: string | null } | null | undefined): boolean {
  const { data: me } = useSession()
  if (!item || !me) return false
  return item.ownerUserId ? item.ownerUserId === me.id : item.createdBy === (me.email ?? me.name)
}

// Shared cross-reference search for the editor's link button: knowledge docs
// AND artifacts — an artifact picked here embeds INLINE as a link at the
// caret (the bottom attachments strip stays for formal doc-artifact ties).
const docSearch: DocSearchFn = async (q) => {
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

const str = (v: unknown): string | undefined => (typeof v === 'string' && v ? v : undefined)

export const Route = createFileRoute('/_app/knowledge')({
  // ?space=<id>&doc=<id> deep-link the tree selection — the URL IS the state.
  //
  // `d` is accepted as an alias for `doc` because SIX link builders spell it
  // that way — this file's own "Copy link" on a doc (twice), the cross-reference
  // picker, the comment notification, and the OKF resource line — and a search
  // key this function does not name is DROPPED. Every one of those links opened
  // the knowledgebase root instead of the document, silently: the same class of
  // bug as a swallowed error, one layer up. Normalizing here fixes all six and
  // any seventh, which chasing the call sites would not. `s` is taken for the
  // same reason before someone writes the matching short form for the space.
  validateSearch: (search: Record<string, unknown>): { space?: string; doc?: string } => {
    const space = str(search.space) ?? str(search.s)
    const doc = str(search.doc) ?? str(search.d)
    return { ...(space ? { space } : {}), ...(doc ? { doc } : {}) }
  },
  component: KnowledgePage,
})

// The knowledgebase — an Outline-style markdown drive. A searchable, nestable
// tree of docs on the left; a WYSIWYG doc with breadcrumb, emoji, table of
// contents, and backlinks in the middle. Official docs feed the org brain;
// agent-kind docs start from an OKF scaffold.
function KnowledgePage() {
  const qc = useQueryClient()
  // "No spaces yet." is a claim that the knowledgebase is EMPTY. A read that
  // failed says nothing about what is in there, so the rejection has to reach
  // the render instead of being swallowed by a `= []` default.
  const spacesQuery = useSpaces()
  const { data: spaces = [], isLoading: spacesLoading } = spacesQuery
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const spaceId = search.space ?? null
  const docId = search.doc ?? null
  // One navigation per selection change — space + doc move together.
  const setLoc = (space: string | null, doc: string | null) =>
    void navigate({ search: { ...(space ? { space } : {}), ...(doc ? { doc } : {}) } })
  const setSpaceId = (id: string | null) => setLoc(id, null)
  const setDocId = (id: string | null) => setLoc(spaceId, id)
  const [creatingSpace, setCreatingSpace] = useState(false)
  const activeSpace = spaces.find((s) => s.id === spaceId) ?? spaces[0]
  const docsQuery = useDocs(activeSpace?.id ?? null)
  const { data: docs = [], isLoading: docsLoading } = docsQuery

  // Landing without a space picks the first one. It must NOT drop the doc: a
  // `?doc=…` link that arrives without a space was rewritten to `?space=…`
  // alone, so the document the link pointed at was thrown away between the
  // click and the first paint, and the reader got the folder overview with no
  // sign anything had been asked for.
  useEffect(() => {
    if (!spaceId && spaces[0])
      void navigate({ search: { space: spaces[0].id, ...(docId ? { doc: docId } : {}) }, replace: true })
  }, [spaces, spaceId, docId, navigate])

  const newSpace = async (name: string) => {
    const { space } = await createSpace(name)
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
    if (space) setSpaceId(space.id)
  }
  const newDoc = async (kind: 'human' | 'agent', parentId: string | null = null) => {
    if (!activeSpace) return
    const { doc } = await createDoc(activeSpace.id, { kind, title: 'Untitled', parentId })
    await qc.invalidateQueries({ queryKey: ['kb-docs', activeSpace.id] })
    if (doc) setDocId(doc.id)
  }
  const move = async (id: string, parentId: string | null, sort: number) => {
    await moveDoc(id, parentId, sort)
    await qc.invalidateQueries({ queryKey: ['kb-docs', activeSpace?.id] })
  }
  const renameSpace = async (id: string, name: string) => {
    await updateSpace(id, { name })
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
  }
  const removeSpace = async (id: string) => {
    await deleteSpace(id)
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
    if (spaceId === id) setLoc(null, null)
  }

  // Jump to a doc from search — switch to its space if needed.
  // Jump to a doc from search — one navigation carries space + doc together.
  const openDoc = (hit: { id: string; spaceId: string; kind?: 'doc' | 'space' }) => {
    setLoc(hit.spaceId, hit.kind === 'space' ? null : hit.id)
  }

  // Right-click menus — shortcuts to actions the sidebar/editors already offer.
  const { openMenu, menu } = useContextMenu()
  // Same createDoc flow as newDoc, but scoped to any space (not just the active one).
  const newDocIn = async (sid: string) => {
    const { doc } = await createDoc(sid, { kind: 'human', title: 'Untitled' })
    await qc.invalidateQueries({ queryKey: ['kb-docs', sid] })
    setLoc(sid, doc ? doc.id : null)
  }
  const spaceMenu = (s: KbSpace): ContextMenuEntry[] => [
    { label: 'Open', onSelect: () => setLoc(s.id, null) },
    { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?space=${s.id}`) },
    { label: 'New doc', onSelect: () => void newDocIn(s.id) },
    'sep',
    {
      label: 'Delete space',
      danger: true,
      onSelect: async () => {
        // Mirrors the space editor's delete confirm.
        if (await confirm({ title: 'Delete folder', message: `Delete "${s.name}" and all its docs?`, confirmLabel: 'Delete', danger: true })) await removeSpace(s.id)
      },
    },
  ]
  const docMenu = (d: KbDocMeta): ContextMenuEntry[] => [
    { label: 'Open', onSelect: () => setLoc(d.spaceId, d.id) },
    { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?space=${d.spaceId}&doc=${d.id}`) },
    'sep',
    {
      label: 'Delete document',
      danger: true,
      onSelect: async () => {
        // Same confirm + deleteDoc flow as the doc editor's kebab menu.
        if (!(await confirm({ title: 'Delete document', message: `Delete "${d.title}"?`, confirmLabel: 'Delete', danger: true }))) return
        await deleteDoc(d.id)
        await qc.invalidateQueries({ queryKey: ['kb-docs', d.spaceId] })
        if (docId === d.id) setDocId(null)
      },
    },
  ]

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex h-full w-72 shrink-0 flex-col border-r border-line-subtle bg-sidebar font-sans">
        <div className="flex h-12 shrink-0 items-center gap-1.5 border-b border-line-subtle px-4">
          <span className="min-w-0 flex-1 truncate text-sm font-semibold text-fg">Knowledge</span>
          <IconButton size="sm" title="New space" onClick={() => setCreatingSpace((v) => !v)}>
            <Plus size={15} />
          </IconButton>
        </div>
        <div className="border-b border-line-subtle p-3">
          <KbSearch onOpen={openDoc} />
        </div>
        {creatingSpace && (
          <div className="border-b border-line-subtle px-3 py-2">
            <Input
              autoFocus
              size="sm"
              placeholder="space name"
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  const v = (e.target as HTMLInputElement).value.trim()
                  if (v) void newSpace(v)
                  setCreatingSpace(false)
                } else if (e.key === 'Escape') setCreatingSpace(false)
              }}
              onBlur={() => setCreatingSpace(false)}
            />
          </div>
        )}
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {spacesLoading ? (
            <SkeletonRows rows={6} className="px-2 py-3" />
          ) : spacesQuery.isError && spacesQuery.data === undefined ? (
            // Unreachable is not deleted. Without this branch an outage reads
            // as somebody having wiped the whole knowledgebase.
            <QueryError
              variant="compact"
              error={spacesQuery.error}
              title="Could not load your spaces"
              onRetry={() => void spacesQuery.refetch()}
            />
          ) : spaces.length === 0 ? (
            // A 200 with no spaces is still a real answer — keep saying so.
            <EmptyState variant="inline" title="No spaces yet." className="px-2 py-6 text-center" />
          ) : (
            spaces.map((s) => (
              <div key={s.id} className="mb-2">
                <SpaceRow
                  space={s}
                  active={activeSpace?.id === s.id}
                  onSelect={() => {
                    setLoc(s.id, null) // open the space's own overview
                  }}
                  onRename={(name) => void renameSpace(s.id, name)}
                  onContextMenu={(e) => openMenu(e, spaceMenu(s))}
                />
                {activeSpace?.id === s.id && (
                  docsLoading ? (
                    // Switching spaces refetches the doc tree — keep its shape.
                    <SkeletonRows rows={6} className="ml-4 mt-1 border-l border-line-subtle py-1 pl-4" />
                  ) : docsQuery.isError && docsQuery.data === undefined ? (
                    // Same lie one level down: a space whose doc list 500s
                    // would otherwise render as a space with nothing in it.
                    <QueryError
                      variant="inline"
                      className="ml-4 mt-1 border-l border-line-subtle py-2 pl-4"
                      error={docsQuery.error}
                      title="Could not load this space"
                      onRetry={() => void docsQuery.refetch()}
                    />
                  ) : (
                    <DocTree docs={docs} activeId={docId} onSelect={setDocId} onNew={newDoc} onMove={move} onDocMenu={(e, d) => openMenu(e, docMenu(d))} />
                  )
                )}
              </div>
            ))
          )}
        </div>
      </aside>

      <main className="min-h-0 min-w-0 flex-1">
        {docId ? (
          <DocEditor key={docId} docId={docId} docs={docs} onDeleted={() => setDocId(null)} onSelect={setDocId} folderName={activeSpace?.name} />
        ) : activeSpace ? (
          // A top-level folder is itself a document: its editable overview.
          <SpaceEditor key={activeSpace.id} spaceId={activeSpace.id} onNewDoc={() => void newDoc('human')} onDeleted={() => void removeSpace(activeSpace.id)} />
        ) : (
          <EmptyState icon="❖" title="Knowledge" hint="Create a space to start writing." />
        )}
      </main>
      {menu}
    </div>
  )
}

// ── Space row ───────────────────────────────────────────────────────────────
// A small kebab (⋯) menu that houses secondary controls (delete, etc.) — the
// per-item settings area, so destructive actions aren't loose in the sidebar.
interface MenuItem {
  label: string
  icon?: LucideIcon
  danger?: boolean
  onClick: () => void
}
function SettingsMenu({ items }: { items: MenuItem[] }) {
  return (
    <DropdownMenu
      align="right"
      className="shrink-0"
      trigger={() => (
        <Button variant="ghost" size="sm" title="More">
          <MoreHorizontal size={14} />
        </Button>
      )}
      items={items.map(
        (it): ContextMenuEntry => ({
          label: it.label,
          icon: it.icon ? <it.icon size={13} /> : undefined,
          danger: it.danger,
          onSelect: it.onClick,
        }),
      )}
    />
  )
}

function SpaceRow({
  space, active, onSelect, onRename, onContextMenu,
}: {
  space: KbSpace
  active: boolean
  onSelect: () => void
  onRename: (name: string) => void
  onContextMenu?: (e: React.MouseEvent) => void
}) {
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(space.name)
  useEffect(() => setName(space.name), [space.name])

  if (editing) {
    return (
      <Input
        size="sm"
        autoFocus
        value={name}
        onChange={(e) => setName(e.target.value)}
        onBlur={() => {
          setEditing(false)
          if (name.trim() && name !== space.name) onRename(name.trim())
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur()
          if (e.key === 'Escape') {
            setName(space.name)
            setEditing(false)
          }
        }}
      />
    )
  }
  return (
    <div
      className={cn('group flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm', active ? 'text-fg' : 'text-muted hover:text-fg')}
      onContextMenu={onContextMenu}
    >
      <button type="button" onClick={onSelect} onDoubleClick={() => setEditing(true)} className="flex min-w-0 flex-1 items-center gap-2 text-left">
        <span>{space.icon ?? '📚'}</span>
        <span className="truncate font-medium">{space.name}</span>
      </button>
    </div>
  )
}

// ── Search ──────────────────────────────────────────────────────────────────
function KbSearch({ onOpen }: { onOpen: (hit: KbSearchHit) => void }) {
  const [q, setQ] = useState('')
  const [hits, setHits] = useState<KbSearchHit[]>([])
  const [open, setOpen] = useState(false)
  useEffect(() => {
    const t = q.trim()
    if (!t) {
      setHits([])
      return
    }
    let live = true
    const id = setTimeout(() => {
      void searchKb(t).then((h) => live && setHits(h))
    }, 180)
    return () => {
      live = false
      clearTimeout(id)
    }
  }, [q])

  return (
    <div className="relative">
      <Search size={13} className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted" />
      <Input
        size="sm"
        value={q}
        onChange={(e) => {
          setQ(e.target.value)
          setOpen(true)
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        placeholder="Search knowledge"
        className="pl-7"
      />
      {open && hits.length > 0 && (
        <div className="absolute left-0 right-0 top-full z-20 mt-1 max-h-80 overflow-y-auto rounded-xl border border-line bg-card p-1 shadow-lg">
          {hits.map((h) => (
            <button
              key={h.id}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault()
                onOpen(h)
                setOpen(false)
              }}
              className="flex w-full flex-col gap-0.5 rounded-lg px-2 py-1.5 text-left hover:bg-sidebar"
            >
              <span className="flex items-center gap-1.5 text-xs text-fg">
                <span>{h.icon ?? '📄'}</span>
                <span className="truncate font-medium">{h.title}</span>
                <span className="ml-auto shrink-0 text-[10px] text-muted">{h.spaceName}</span>
              </span>
              {h.snippet && <span className="line-clamp-2 text-[11px] text-muted" dangerouslySetInnerHTML={{ __html: h.snippet }} />}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

// ── Nested tree ─────────────────────────────────────────────────────────────
type DropPos = 'before' | 'after' | 'inside'

function DocTree({
  docs, activeId, onSelect, onNew, onMove, onDocMenu,
}: {
  docs: KbDocMeta[]
  activeId: string | null
  onSelect: (id: string) => void
  onNew: (k: 'human' | 'agent', parentId?: string | null) => void
  onMove: (id: string, parentId: string | null, sort: number) => void
  onDocMenu: (e: React.MouseEvent, d: KbDocMeta) => void
}) {
  const byParent = useMemo(() => {
    const m = new Map<string | null, KbDocMeta[]>()
    for (const d of docs) {
      const k = d.parentId ?? null
      if (!m.has(k)) m.set(k, [])
      m.get(k)!.push(d)
    }
    for (const list of m.values()) list.sort((a, b) => a.sort - b.sort || a.title.localeCompare(b.title))
    return m
  }, [docs])

  const [dragId, setDragId] = useState<string | null>(null)

  // Reparent/reorder: drop 'inside' T nests under it; 'before'/'after' T orders
  // among T's siblings. Sibling order is persisted by reindexing that group.
  const drop = (targetId: string, pos: DropPos) => {
    if (!dragId || dragId === targetId) return
    const target = docs.find((d) => d.id === targetId)
    if (!target) return
    if (pos === 'inside') {
      const kids = byParent.get(targetId) ?? []
      onMove(dragId, targetId, kids.length)
      return
    }
    const parent = target.parentId ?? null
    const sibs = (byParent.get(parent) ?? []).filter((d) => d.id !== dragId)
    const idx = sibs.findIndex((d) => d.id === targetId)
    const at = pos === 'before' ? idx : idx + 1
    sibs.splice(at, 0, docs.find((d) => d.id === dragId)!)
    sibs.forEach((d, i) => {
      if (d.id === dragId) onMove(dragId, parent, i)
      else if (d.sort !== i) void moveDoc(d.id, parent, i)
    })
  }

  const roots = byParent.get(null) ?? []
  return (
    <div
      className="ml-2 mt-0.5 space-y-0.5 border-l border-line-subtle pl-2"
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        // Drop in the empty tree area → move to root (append).
        e.preventDefault()
        if (dragId) onMove(dragId, null, roots.length)
        setDragId(null)
      }}
    >
      {roots.map((d) => (
        <DocRow
          key={d.id}
          doc={d}
          depth={0}
          byParent={byParent}
          activeId={activeId}
          onSelect={onSelect}
          onNew={onNew}
          dragId={dragId}
          setDragId={setDragId}
          onDropRel={drop}
          onDocMenu={onDocMenu}
        />
      ))}
      <div className="flex gap-1 pt-1">
        <button type="button" onClick={() => onNew('human')} className="flex items-center gap-1 rounded-md px-2 py-1 text-[11px] text-muted hover:text-accent">
          <Plus size={11} /> Doc
        </button>
        <button type="button" onClick={() => onNew('agent')} className="flex items-center gap-1 rounded-md px-2 py-1 text-[11px] text-muted hover:text-accent" title="OKF-structured for agents">
          <Bot size={11} /> Agent doc
        </button>
      </div>
    </div>
  )
}

function DocRow({
  doc, depth, byParent, activeId, onSelect, onNew, dragId, setDragId, onDropRel, onDocMenu,
}: {
  doc: KbDocMeta
  depth: number
  byParent: Map<string | null, KbDocMeta[]>
  activeId: string | null
  onSelect: (id: string) => void
  onNew: (k: 'human' | 'agent', parentId?: string | null) => void
  dragId: string | null
  setDragId: (id: string | null) => void
  onDropRel: (targetId: string, pos: DropPos) => void
  onDocMenu: (e: React.MouseEvent, d: KbDocMeta) => void
}) {
  const kids = byParent.get(doc.id) ?? []
  const [expanded, setExpanded] = useState(true)
  const [pos, setPos] = useState<DropPos | null>(null)
  const hasKids = kids.length > 0

  return (
    <div>
      <div
        draggable
        onDragStart={(e) => {
          e.stopPropagation()
          setDragId(doc.id)
          e.dataTransfer.effectAllowed = 'move'
        }}
        onDragEnd={() => setDragId(null)}
        onDragOver={(e) => {
          e.preventDefault()
          e.stopPropagation()
          if (!dragId || dragId === doc.id) return
          // Top third → before, bottom third → after, middle → nest inside.
          const r = e.currentTarget.getBoundingClientRect()
          const y = (e.clientY - r.top) / r.height
          setPos(y < 0.3 ? 'before' : y > 0.7 ? 'after' : 'inside')
        }}
        onDragLeave={() => setPos(null)}
        onDrop={(e) => {
          e.preventDefault()
          e.stopPropagation()
          if (pos) onDropRel(doc.id, pos)
          setPos(null)
          setDragId(null)
        }}
        onContextMenu={(e) => onDocMenu(e, doc)}
        className={cn(
          'group relative flex items-center gap-1 rounded-md py-1 pr-1 text-xs',
          activeId === doc.id ? 'bg-card text-fg' : 'text-muted hover:text-fg',
          pos === 'inside' && 'ring-1 ring-accent/60',
        )}
        style={{ paddingLeft: depth * 12 + 4 }}
      >
        {pos === 'before' && <span className="absolute inset-x-1 top-0 h-0.5 rounded bg-accent" />}
        {pos === 'after' && <span className="absolute inset-x-1 bottom-0 h-0.5 rounded bg-accent" />}
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className={cn('shrink-0 rounded p-0.5 hover:bg-sidebar', !hasKids && 'invisible')}
        >
          <ChevronRight size={12} className={cn('transition-transform', expanded && 'rotate-90')} />
        </button>
        <button type="button" onClick={() => onSelect(doc.id)} className="flex min-w-0 flex-1 items-center gap-1.5 text-left">
          {doc.icon ? <span className="shrink-0 text-[13px] leading-none">{doc.icon}</span> : doc.kind === 'agent' ? <Bot size={12} className="shrink-0" /> : <FileText size={12} className="shrink-0" />}
          <span className="min-w-0 flex-1 truncate">{doc.title}</span>
          {doc.official && <Star size={11} className="shrink-0 text-[color:var(--theme-warning)]" />}
        </button>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation()
            onNew('human', doc.id)
            setExpanded(true)
          }}
          title="New nested doc"
          className="shrink-0 rounded p-0.5 text-muted opacity-0 hover:bg-sidebar hover:text-accent group-hover:opacity-100"
        >
          <Plus size={12} />
        </button>
      </div>
      {hasKids && expanded && (
        <div>
          {kids.map((k) => (
            <DocRow
              key={k.id}
              doc={k}
              depth={depth + 1}
              byParent={byParent}
              activeId={activeId}
              onSelect={onSelect}
              onNew={onNew}
              dragId={dragId}
              setDragId={setDragId}
              onDropRel={onDropRel}
              onDocMenu={onDocMenu}
            />
          ))}
        </div>
      )}
    </div>
  )
}

// ── Page skeleton ───────────────────────────────────────────────────────────
// Matches the doc/space editor layout (breadcrumb, toolbar with icon + title +
// buttons, centered prose column) so the swap to real content doesn't jump.
const PROSE_WIDTHS = ['100%', '92%', '97%', '86%', '95%', '73%', '100%', '90%', '96%', '88%', '94%', '58%']
function DocPageSkeleton({ breadcrumb = false, bars = 10 }: { breadcrumb?: boolean; bars?: number }) {
  return (
    <div aria-hidden className="flex h-full min-h-0 flex-col">
      {breadcrumb && (
        <div className="border-b border-line-subtle px-6 pb-2 pt-3">
          <Skeleton className="h-2.5 w-44 rounded-full" />
        </div>
      )}
      <div className="flex items-center gap-3 border-b border-line-subtle px-6 py-4">
        <Skeleton className="h-7 w-7 shrink-0" />
        <Skeleton className="h-5 w-64 max-w-[40%] rounded-full" delay={0.08} />
        <span className="ml-auto flex shrink-0 gap-2">
          <Skeleton className="h-7 w-20" delay={0.16} />
          <Skeleton className="h-7 w-28" delay={0.24} />
        </span>
      </div>
      <div className="mx-auto w-full max-w-[46rem] flex-1 space-y-3.5 px-6 py-8">
        {PROSE_WIDTHS.slice(0, bars).map((w, i) => (
          <div key={i} style={{ width: w }}>
            <Skeleton className="h-3 w-full rounded-full" delay={i * 0.08} />
          </div>
        ))}
      </div>
    </div>
  )
}

// Editor shell: normal fill vs a fullscreen takeover — same column layout.
const editorShell = (fullscreen: boolean) =>
  cn('flex min-h-0 flex-col', fullscreen ? 'fixed inset-0 z-50 bg-surface' : 'h-full')

// ── Space overview (top-level folder = document) ────────────────────────────
function SpaceEditor({ spaceId, onNewDoc, onDeleted }: { spaceId: string; onNewDoc: () => void; onDeleted: () => void }) {
  const qc = useQueryClient()
  const spaceQuery = useSpace(spaceId)
  const space = spaceQuery.data
  const editorRef = useRef<RichEditorHandle>(null)
  const bodyRef = useRef<HTMLDivElement>(null)
  const [name, setName] = useState('')
  const [emojiOpen, setEmojiOpen] = useState(false)
  const [fullscreen, setFullscreen] = useState(false)
  const [shareOpen, setShareOpen] = useState(false)
  const [showToc, setShowToc] = useState(false)
  const [showHistory, setShowHistory] = useState(false)
  const [seed, setSeed] = useState(0)
  const [mode, setMode] = useState<'read' | 'edit'>('read')
  const [museSel, setMuseSel] = useState<{ text: string; source: 'read' | 'editor' } | null>(null)
  const { openMenu: openSpaceMenu, menu: spaceMenu } = useContextMenu()
  const isOwner = useIsOwner(space)
  const initMode = useRef(false)
  const headings = useMemo(() => parseHeadings(space?.body ?? ''), [space?.body])
  const scrollToHeading = (index: number) => {
    bodyRef.current?.querySelectorAll('.tiptap h1, .tiptap h2, .tiptap h3')[index]?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }
  useEffect(() => {
    if (space) setName(space.name)
    if (space && !initMode.current) {
      initMode.current = true
      setMode(space.body.trim() ? 'read' : 'edit')
    }
  }, [space])
  useEffect(() => {
    if (!fullscreen) return
    const h = (e: KeyboardEvent) => e.key === 'Escape' && setFullscreen(false)
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [fullscreen])

  const save = async (patch: Parameters<typeof updateSpace>[1]) => {
    await updateSpace(spaceId, patch)
    await qc.invalidateQueries({ queryKey: ['kb-space', spaceId] })
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
  }
  const saveBody = () => save({ name: name.trim() || 'Untitled', body: editorRef.current?.getMarkdown() ?? space?.body ?? '' })

  // `if (!space)` used to cover all three answers at once, so a 500 and a
  // deleted folder both shimmered nine placeholder bars for ever. `useSpace`
  // rejects on failure and resolves to `null` for a real 404 — the two ARE
  // distinguishable here, this branch simply never looked.
  if (spaceQuery.isError && space === undefined)
    return (
      <QueryError
        error={spaceQuery.error}
        title="Could not load this folder"
        onRetry={() => void spaceQuery.refetch()}
      />
    )
  if (space === null)
    return <EmptyState icon="⧉" title="Folder not found" hint="It may have been deleted, or you don’t have access." />
  if (!space) return <DocPageSkeleton bars={9} />

  return (
    <div className={editorShell(fullscreen)}>
      <div className="flex flex-wrap items-center gap-2 border-b border-line-subtle px-6 py-3">
        <div className="relative shrink-0">
          <button type="button" onClick={() => setEmojiOpen((v) => !v)} className="rounded-lg px-1 text-xl leading-none hover:bg-card" title="Set icon">
            {space.icon ?? '📚'}
          </button>
          {emojiOpen && (
            <EmojiPicker
              onPick={(e) => {
                void save({ icon: e })
                setEmojiOpen(false)
              }}
              onClear={() => {
                void save({ icon: null })
                setEmojiOpen(false)
              }}
              onClose={() => setEmojiOpen(false)}
            />
          )}
        </div>
        {mode === 'edit' ? (
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => name.trim() && name !== space.name && void save({ name: name.trim() })}
            onKeyDown={inlineEditKeys(() => setName(space.name))}
            className="min-w-0 flex-1 border-0 bg-transparent text-xl font-semibold focus:border-0"
            placeholder="Space name"
          />
        ) : (
          <div className="min-w-0 flex-1">
            <h1 className="truncate font-sans text-lg font-semibold text-fg">{space.name}</h1>
            <div className="truncate text-[11px] text-muted">space overview</div>
          </div>
        )}
        <Chip className="font-normal">Folder</Chip>
        <Segmented
          size="xs"
          options={[{ id: 'read' as const, label: 'Read' }, { id: 'edit' as const, label: 'Edit' }]}
          value={mode}
          onChange={(m) => {
            if (m === 'read' && mode === 'edit') void saveBody()
            setMode(m)
          }}
        />
        <Button variant="ghost" size="sm" className="shrink-0" title="Share & permissions" onClick={() => setShareOpen(true)}>
          <VisibilityIcon v={space.visibility} /> <span className="ml-1.5 capitalize">{space.visibility}</span>
        </Button>
        <span className="mx-0.5 h-4 shrink-0 border-l border-line-subtle" />
        <Button variant={showToc ? 'outline' : 'ghost'} size="sm" className="shrink-0" title="Table of contents" onClick={() => setShowToc((v) => !v)}>
          <ListTree size={14} />
        </Button>
        <Button variant="ghost" size="sm" className="shrink-0" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onClick={() => setFullscreen((v) => !v)}>
          {fullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
        </Button>
        <Button variant="outline" size="sm" className="shrink-0" onClick={onNewDoc}>
          <Plus size={13} className="mr-1" /> New
        </Button>
        <SettingsMenu
          items={[
            { label: showHistory ? 'Hide history' : 'Version history', icon: History, onClick: () => setShowHistory((v) => !v) },
            {
              label: 'Delete folder',
              icon: Trash2,
              danger: true,
              onClick: async () => {
                if (await confirm({ title: 'Delete folder', message: `Delete "${space.name}" and all its docs?`, confirmLabel: 'Delete', danger: true })) onDeleted()
              },
            },
          ]}
        />
      </div>
      <div ref={bodyRef} className="flex min-h-0 flex-1">
        {mode === 'edit' ? (
          <div
            className="flex min-w-0 flex-1 flex-col"
            onContextMenu={(e) => {
              const sel = editorRef.current?.getSelectionText() ?? ''
              const inTable = editorRef.current?.isInTable() ?? false
              openSpaceMenu(e, [
                { label: 'Bold', disabled: !sel, onSelect: () => editorRef.current?.toggleMark('bold') },
                { label: 'Italic', disabled: !sel, onSelect: () => editorRef.current?.toggleMark('italic') },
                { label: 'Strikethrough', disabled: !sel, onSelect: () => editorRef.current?.toggleMark('strike') },
                { label: 'Inline code', disabled: !sel, onSelect: () => editorRef.current?.toggleMark('code') },
                'sep',
                ...(inTable
                  ? ([
                      { label: 'Add row below', onSelect: () => editorRef.current?.tableCommand('addRowAfter') },
                      { label: 'Add row above', onSelect: () => editorRef.current?.tableCommand('addRowBefore') },
                      { label: 'Add column right', onSelect: () => editorRef.current?.tableCommand('addColumnAfter') },
                      { label: 'Add column left', onSelect: () => editorRef.current?.tableCommand('addColumnBefore') },
                      { label: 'Delete row', onSelect: () => editorRef.current?.tableCommand('deleteRow') },
                      { label: 'Delete column', onSelect: () => editorRef.current?.tableCommand('deleteColumn') },
                      { label: 'Delete table', danger: true, onSelect: () => editorRef.current?.tableCommand('deleteTable') },
                      'sep',
                    ] as const)
                  : ([{ label: 'Insert table', onSelect: () => editorRef.current?.tableCommand('insertTable') }, 'sep'] as const)),
                { label: 'Ask Muse about selection', disabled: !sel, onSelect: () => setMuseSel({ text: sel, source: 'editor' }) },
                { label: 'Copy selection', disabled: !sel, onSelect: () => void navigator.clipboard.writeText(sel) },
              ])
            }}
          >
            <RichEditor
              key={`${spaceId}-${seed}`}
              ref={editorRef}
              value={space.body}
              docSearch={docSearch}
              slash
              prose
              autosave
              onSave={() => void saveBody()}
              placeholder="Write an overview for this space: what lives here, how it's organized"
              fill
              className="min-w-0 flex-1"
            />
            <MuseBar
              context={`Overview document for the knowledge space “${name || space.name}”.`}
              currentText={() => editorRef.current?.getMarkdown() ?? space.body}
              selection={museSel?.text ?? null}
              onClearSelection={() => setMuseSel(null)}
              surgical={!!museSel}
              onAccept={async (md) => {
                await save({ body: md })
                setSeed((v) => v + 1)
                setMuseSel(null)
              }}
              onAcceptSelection={async (replacement) => {
                if (!museSel) return
                if (museSel.source === 'editor') {
                  editorRef.current?.replaceSelection(replacement)
                  void saveBody()
                } else {
                  const body = editorRef.current?.getMarkdown() ?? space.body
                  await save({ body: body.replace(museSel.text, replacement) })
                  setSeed((v) => v + 1)
                }
                setMuseSel(null)
              }}
            />
          </div>
        ) : (
          <div className="flex min-w-0 flex-1 flex-col">
          <div
            className="re-prose min-w-0 flex-1 overflow-y-auto"
            onContextMenu={(e) => {
              const sel = window.getSelection()?.toString().trim() ?? ''
              openSpaceMenu(e, [
                { label: 'Copy text', disabled: !sel && !space.body, onSelect: () => void navigator.clipboard.writeText(sel || space.body) },
                { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?space=${spaceId}`) },
                ...(sel ? [{ label: 'Ask Muse about selection', onSelect: () => setMuseSel({ text: sel, source: 'read' }) }] : []),
              ])
            }}
          >
            {space.body.trim() ? (
              <Markdown className="tiptap">{space.body}</Markdown>
            ) : (
              <div className="mx-auto max-w-[46rem] px-6 py-8">
                <button type="button" onClick={() => setMode('edit')} className="text-sm text-muted hover:text-fg">
                  No overview yet. Click to describe this space.
                </button>
              </div>
            )}
          </div>
          <MuseBar
            context={`Overview document for the knowledge space “${space.name}”.`}
            currentText={() => space.body}
            selection={museSel?.text ?? null}
            onClearSelection={() => setMuseSel(null)}
            surgical={!!museSel && space.body.includes(museSel.text)}
            onAccept={async (md) => {
              await save({ body: md })
              setMuseSel(null)
            }}
            onAcceptSelection={async (replacement) => {
              if (!museSel) return
              await save({ body: space.body.replace(museSel.text, replacement) })
              setMuseSel(null)
            }}
          />
          </div>
        )}
        {showToc && (
          <TocPanel headings={headings} onJump={scrollToHeading} onClose={() => setShowToc(false)} emptyText="No headings yet." />
        )}
        {showHistory && (
          <div className="w-64 shrink-0 overflow-y-auto border-l border-line-subtle p-3">
            <HistoryRail
              kind="kb-space"
              id={spaceId}
              onRestore={async (content) => {
                const m = /^#\s+(.*)\n+([\s\S]*)$/.exec(content)
                const nm = m ? m[1]!.trim() : name
                const b = m ? m[2]! : content
                setName(nm)
                await save({ name: nm, body: b })
                setSeed((n) => n + 1)
              }}
            />
          </div>
        )}
      </div>
      {spaceMenu}
      <PermissionsModal
        open={shareOpen}
        onClose={() => setShareOpen(false)}
        kind="spaces"
        id={spaceId}
        label={space.name}
        visibility={space.visibility}
        editPolicy={space.editPolicy}
        publicSlug={space.publicSlug}
        canManage={isOwner}
        onSave={(patch) => save(patch)}
      />
    </div>
  )
}

// ── Editor ──────────────────────────────────────────────────────────────────
interface Heading {
  level: number
  text: string
}
function parseHeadings(md: string): Heading[] {
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

// The table-of-contents side rail — identical in the space and doc editors.
function TocPanel({
  headings, onJump, onClose, emptyText,
}: {
  headings: Heading[]
  onJump: (index: number) => void
  onClose: () => void
  emptyText: string
}) {
  return (
    <div className="w-56 shrink-0 overflow-y-auto border-l border-line-subtle p-3">
      <div className="mb-2 flex items-center justify-between text-[11px] uppercase tracking-wide text-muted">
        <span>Contents</span>
        <CloseButton onClick={onClose} size={12} className="p-0 hover:bg-transparent" />
      </div>
      {headings.length === 0 ? (
        <EmptyState variant="inline" title={emptyText} />
      ) : (
        <div className="space-y-0.5">
          {headings.map((h, i) => (
            <button
              key={i}
              type="button"
              onClick={() => onJump(i)}
              className="block w-full truncate text-left font-sans text-xs text-muted hover:text-fg"
              style={{ paddingLeft: (h.level - 1) * 10 }}
            >
              {h.text || 'Untitled'}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

function DocEditor({
  docId, docs, onDeleted, onSelect, folderName,
}: {
  docId: string
  docs: KbDocMeta[]
  onDeleted: () => void
  onSelect: (id: string) => void
  folderName?: string
}) {
  const qc = useQueryClient()
  const docQuery = useDoc(docId)
  const doc = docQuery.data
  const { data: me } = useSession()
  // "Nothing links here" is a claim about the whole knowledge base; a failed
  // read must not make it.
  const backlinksList = listQuery(useBacklinks(docId), { title: 'Could not load backlinks', variant: 'inline' })
  const backlinks = backlinksList.rows
  const backlinksLoading = backlinksList.pending
  const editorRef = useRef<RichEditorHandle>(null)
  const bodyRef = useRef<HTMLDivElement>(null)
  const [title, setTitle] = useState('')
  const [dirty, setDirty] = useState(false)
  const [showHistory, setShowHistory] = useState(false)
  const [showToc, setShowToc] = useState(false)
  const [emojiOpen, setEmojiOpen] = useState(false)
  const [fullscreen, setFullscreen] = useState(false)
  const [shareOpen, setShareOpen] = useState(false)
  const [seed, setSeed] = useState(0) // bump to remount the editor (e.g. after restore)
  const ownFallback = useIsOwner(doc)
  // Sharing governance: server-computed (covers agent-created docs any
  // agent-user may govern); fall back to plain ownership for older payloads.
  const isOwner = doc?.governs ?? ownFallback
  // Authored docs open in read mode (like tickets); empty ones open in edit.
  const [mode, setMode] = useState<'read' | 'edit'>('read')
  const [showComments, setShowComments] = useState(false)
  const [focusThread, setFocusThread] = useState<string | null>(null)
  const [museSel, setMuseSel] = useState<{ text: string; source: 'read' | 'editor' } | null>(null)
  const [okfOpen, setOkfOpen] = useState(false)
  const { openMenu: openDocMenu, menu: docMenu } = useContextMenu()
  const readRef = useRef<HTMLDivElement>(null)
  const [pendingQuote, setPendingQuote] = useState<string | null>(null)
  const [selPop, setSelPop] = useState<{ x: number; y: number; quote: string } | null>(null)
  // Presence drives the multiplayer avatars and the read-mode auto-refresh.
  // Defaulted it says "you are alone in here" during an outage, which is when
  // two people are most likely to overwrite each other.
  const presenceList = listQuery(useDocLive(docId, mode), { title: 'Could not see who else is here', variant: 'inline' })
  const presence = presenceList.rows
  const commentsQuery = useDocComments(docId)
  const { data: comments = [] } = commentsQuery
  const openThreads = comments.filter((c) => !c.parentId && !c.resolved).length
  const otherEditors = presence.filter((p) => p.userId !== me?.id && p.mode === 'edit')
  const initMode = useRef(false)
  useEffect(() => {
    if (doc) setTitle(doc.title)
    if (doc && !initMode.current) {
      initMode.current = true
      setMode(doc.body.trim() ? 'read' : 'edit')
    }
  }, [doc])

  // Multiplayer read freshness: while others are here, the rendered doc
  // follows their saves (edit mode never yanks your buffer).
  useEffect(() => {
    if (mode !== 'read' || presence.length <= 1) return
    const t = setInterval(() => void qc.invalidateQueries({ queryKey: ['kb-doc', docId] }), 10_000)
    return () => clearInterval(t)
  }, [mode, presence.length, docId, qc])

  // Quote-anchored highlights: after render, find each OPEN thread's quote in
  // the read surface and wrap it in a clickable mark. Idempotent — old marks
  // unwrap first; single-text-node matches only (quotes are plain sentences).
  useEffect(() => {
    const host = readRef.current
    if (!host || mode !== 'read') return
    for (const old of Array.from(host.querySelectorAll('[data-kb-mark]'))) {
      const parent = old.parentNode
      if (!parent) continue
      while (old.firstChild) parent.insertBefore(old.firstChild, old)
      parent.removeChild(old)
      parent.normalize()
    }
    const targets = comments.filter((c) => !c.parentId && !c.resolved && c.quote?.trim())
    for (const c of targets) {
      const quote = c.quote!.replace(/\s+/g, ' ').trim()
      const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT)
      let node: Text | null
      while ((node = walker.nextNode() as Text | null)) {
        if (node.parentElement?.closest('[data-kb-mark]')) continue
        const text = node.textContent ?? ''
        const idx = text.replace(/\s+/g, ' ').indexOf(quote)
        if (idx === -1) continue
        // Map the normalized index back — safe when the node has no runs of
        // whitespace; bail to a plain indexOf otherwise.
        const rawIdx = text.indexOf(c.quote!.trim()) !== -1 ? text.indexOf(c.quote!.trim()) : idx
        try {
          const range = document.createRange()
          range.setStart(node, rawIdx)
          range.setEnd(node, Math.min(rawIdx + c.quote!.trim().length, text.length))
          const mark = document.createElement('span')
          mark.className = 'kb-comment-mark'
          mark.dataset.kbMark = c.id
          mark.title = `${c.author}: ${c.content.slice(0, 80)}`
          range.surroundContents(mark)
        } catch {
          /* range crossed an element boundary — skip this quote */
        }
        break
      }
    }
  }, [mode, comments, doc?.body])

  // Esc leaves fullscreen (unless a menu/popup is open and swallows it first).
  useEffect(() => {
    if (!fullscreen) return
    const h = (e: KeyboardEvent) => e.key === 'Escape' && setFullscreen(false)
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [fullscreen])

  const headings = useMemo(() => parseHeadings(doc?.body ?? ''), [doc?.body])

  // Breadcrumb: walk parentId up from this doc.
  const trail = useMemo(() => {
    const chain: KbDocMeta[] = []
    let cur = docs.find((d) => d.id === docId)
    const seen = new Set<string>()
    while (cur && !seen.has(cur.id)) {
      seen.add(cur.id)
      chain.unshift(cur)
      cur = cur.parentId ? docs.find((d) => d.id === cur!.parentId) : undefined
    }
    return chain
  }, [docs, docId])

  const save = async (patch: Parameters<typeof saveDoc>[1]) => {
    await saveDoc(docId, patch)
    await qc.invalidateQueries({ queryKey: ['kb-doc', docId] })
    await qc.invalidateQueries({ queryKey: ['kb-docs', doc?.spaceId] })
    setDirty(false)
  }
  const saveBody = () => save({ title, body: editorRef.current?.getMarkdown() ?? doc?.body ?? '' })

  // Scroll the rendered editor to the Nth heading (headings render in order).
  const scrollToHeading = (index: number) => {
    const nodes = bodyRef.current?.querySelectorAll('.tiptap h1, .tiptap h2, .tiptap h3')
    nodes?.[index]?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  // Same three answers, same collapse: twelve skeleton bars that never resolve,
  // for a 500 and for a document that genuinely no longer exists.
  if (docQuery.isError && doc === undefined)
    return (
      <QueryError
        error={docQuery.error}
        title="Could not load this document"
        onRetry={() => void docQuery.refetch()}
      />
    )
  if (doc === null)
    return <EmptyState icon="⧉" title="Document not found" hint="It may have been deleted, or you don’t have access." />
  if (!doc) return <DocPageSkeleton breadcrumb bars={12} />

  return (
    <div className={editorShell(fullscreen)}>
      {/* Breadcrumb */}
      <div className="flex items-center gap-1 border-b border-line-subtle px-6 pt-2 text-[11px] text-muted">
        {trail.map((d, i) => (
          <span key={d.id} className="flex items-center gap-1">
            {i > 0 && <ChevronRight size={11} className="opacity-50" />}
            <button
              type="button"
              onClick={() => d.id !== docId && onSelect(d.id)}
              className={cn('max-w-[12rem] truncate font-sans', d.id === docId ? 'text-fg' : 'hover:text-fg')}
            >
              {d.icon ? `${d.icon} ` : ''}
              {d.title}
            </button>
          </span>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-2 border-b border-line-subtle px-6 py-3">
        <div className="relative shrink-0">
          <button
            type="button"
            onClick={() => setEmojiOpen((v) => !v)}
            className="rounded-lg px-1 text-xl leading-none hover:bg-card"
            title="Set icon"
          >
            {doc.icon ?? '📄'}
          </button>
          {emojiOpen && (
            <EmojiPicker
              onPick={(e) => {
                void save({ icon: e })
                setEmojiOpen(false)
              }}
              onClear={() => {
                void save({ icon: null })
                setEmojiOpen(false)
              }}
              onClose={() => setEmojiOpen(false)}
            />
          )}
        </div>
        {mode === 'edit' ? (
          <Input
            value={title}
            onChange={(e) => {
              setTitle(e.target.value)
              setDirty(true)
            }}
            onBlur={() => dirty && void saveBody()}
            onKeyDown={inlineEditKeys(() => doc && setTitle(doc.title))}
            className="min-w-0 flex-1 border-0 bg-transparent text-lg font-semibold focus:border-0"
            placeholder="Untitled"
          />
        ) : (
          <div className="min-w-0 flex-1">
            <h1 className="truncate font-sans text-lg font-semibold text-fg">{doc.title}</h1>
            <div className="truncate text-[11px] text-muted">
              edited {relativeTime(doc.updatedAt)}
              {doc.updatedBy ? ` by ${doc.updatedBy}` : ''}
            </div>
          </div>
        )}
        {doc.okf && (
          <button
            type="button"
            onClick={() => setOkfOpen(true)}
            className="shrink-0 rounded border border-accent/40 bg-accent/10 px-1.5 text-[10px] uppercase tracking-wide text-accent transition-colors hover:bg-accent/20"
            title="This promoted document carries an agent-facing OKF summary, maintained by the Librarian. Click to view."
          >
            OKF
          </button>
        )}
        {presenceList.notice}
        {/* Who's here: green ring = editing right now. */}
        {presence.length > 1 && (
          <div className="flex shrink-0 -space-x-1.5">
            {presence.slice(0, 5).map((p) => (
              <span key={p.userId} className="relative" title={`${p.name}${p.mode === 'edit' ? ' — editing' : ''}`}>
                <Avatar
                  name={p.name}
                  className={cn('h-6 w-6 text-[10px] ring-2 ring-surface', p.mode === 'edit' && 'ring-[color:var(--theme-success)]')}
                />
                {p.mode === 'edit' && (
                  <Pencil size={8} className="absolute -bottom-0.5 -right-0.5 rounded-full bg-surface text-[color:var(--theme-success)]" />
                )}
              </span>
            ))}
          </div>
        )}
        {mode === 'edit' && otherEditors.length > 0 && (
          <span
            className="shrink-0 rounded-full bg-[color:var(--theme-warning)]/15 px-2 py-0.5 text-[11px] text-[color:var(--theme-warning)]"
            title="Someone else is editing too — last save wins, so coordinate or take turns"
          >
            also editing: {otherEditors.map((p) => p.name).join(', ')}
          </span>
        )}
        <Button
          variant={showComments ? 'outline' : 'ghost'}
          size="sm"
          className="shrink-0"
          title="Comments"
          onClick={() => setShowComments((v) => !v)}
        >
          <MessageSquareText size={14} />
          {openThreads > 0 && <span className="ml-1 text-[11px] text-accent">{openThreads}</span>}
        </Button>
        {/* Read / Edit toggle — authored docs open in read mode. */}
        <Segmented
          size="xs"
          options={[{ id: 'read' as const, label: 'Read' }, { id: 'edit' as const, label: 'Edit' }]}
          value={mode}
          onChange={(m) => {
            if (m === 'read' && mode === 'edit') void saveBody() // capture edits on exit
            setMode(m)
          }}
        />
        {/* Sharing + promotion: one quiet cluster. The Official pill is a
            STATE — promoting confirms once, demoting confirms twice (it pulls
            the doc out of what grounds every agent). */}
        <Button variant="ghost" size="sm" className="shrink-0" title="Share & permissions" onClick={() => setShareOpen(true)}>
          <VisibilityIcon v={doc.visibility} /> <span className="ml-1.5 capitalize">{doc.visibility}</span>
        </Button>
        {doc.official ? (
          <button
            type="button"
            className="flex shrink-0 items-center gap-1 rounded-full border border-[color:var(--theme-warning)]/50 bg-[color:var(--theme-warning)]/10 px-2.5 py-1 text-xs text-[color:var(--theme-warning)] transition-colors hover:bg-[color:var(--theme-warning)]/20"
            title="Official — grounds every agent via the org brain. Click to demote (double confirm)."
            onClick={async () => {
              if (!(await confirm({ title: 'Demote from official?', message: `“${doc.title}” currently grounds every agent through the org brain. Demoting removes it from retrieval.`, confirmLabel: 'Continue' }))) return
              if (!(await confirm({ title: 'Really demote?', message: 'Agents stop grounding on this document immediately. This is the final confirmation.', confirmLabel: 'Demote', danger: true }))) return
              void save({ official: false })
            }}
          >
            <Star size={12} fill="currentColor" /> Official
          </button>
        ) : (
          <Button
            variant="ghost"
            size="sm"
            className="shrink-0"
            title="Promote to official — indexed into the organization brain agents ground on"
            onClick={async () => {
              if (await confirm({ title: 'Promote to official?', message: `“${doc.title}” will be indexed into the org brain and ground every agent's answers.`, confirmLabel: 'Promote' })) {
                void save({ official: true })
              }
            }}
          >
            <Star size={13} className="mr-1" /> Promote
          </Button>
        )}
        <BrainRoutingSelect value={doc.ragRouting} canEdit={isOwner} onChange={(ragRouting) => void save({ ragRouting })} />
        <span className="mx-0.5 h-4 shrink-0 border-l border-line-subtle" />
        <Button variant={showToc ? 'outline' : 'ghost'} size="sm" className="shrink-0" title="Table of contents" onClick={() => setShowToc((v) => !v)}>
          <ListTree size={14} />
        </Button>
        <Button variant="ghost" size="sm" className="shrink-0" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onClick={() => setFullscreen((v) => !v)}>
          {fullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
        </Button>
        <SettingsMenu
          items={[
            { label: showHistory ? 'Hide history' : 'Version history', icon: History, onClick: () => setShowHistory((v) => !v) },
            {
              label: 'Delete document',
              icon: Trash2,
              danger: true,
              onClick: async () => {
                if (!(await confirm({ title: 'Delete document', message: `Delete "${doc.title}"?`, confirmLabel: 'Delete', danger: true }))) return
                await deleteDoc(docId)
                await qc.invalidateQueries({ queryKey: ['kb-docs', doc.spaceId] })
                onDeleted()
              },
            },
          ]}
        />
      </div>

      {doc.visibility === 'public' && doc.publicSlug && (
        <div className="flex items-center gap-2 border-b border-line-subtle bg-card/40 px-6 py-1.5 text-xs text-muted">
          <Globe size={12} /> Public link:
          <code className="text-fg">{typeof window !== 'undefined' ? `${window.location.origin}/kb/${doc.publicSlug}` : `/kb/${doc.publicSlug}`}</code>
        </div>
      )}

      <div ref={bodyRef} className="flex min-h-0 flex-1">
        {mode === 'edit' ? (
          // Flush page surface: the editor fills the panel, text wraps to a
          // comfortable centered measure, and it autosaves as you type.
          <div
            className="flex min-w-0 flex-1 flex-col"
            onContextMenu={(e) => {
              // Our menu replaces the native one inside the editor: formatting
              // + table structure on the selection, Muse, copy. (Global
              // suppression exempts contenteditable, so this is deliberate.)
              const sel = editorRef.current?.getSelectionText() ?? ''
              const inTable = editorRef.current?.isInTable() ?? false
              openDocMenu(e, [
                { label: 'Bold', disabled: !sel, onSelect: () => editorRef.current?.toggleMark('bold') },
                { label: 'Italic', disabled: !sel, onSelect: () => editorRef.current?.toggleMark('italic') },
                { label: 'Strikethrough', disabled: !sel, onSelect: () => editorRef.current?.toggleMark('strike') },
                { label: 'Inline code', disabled: !sel, onSelect: () => editorRef.current?.toggleMark('code') },
                'sep',
                ...(inTable
                  ? ([
                      { label: 'Add row below', onSelect: () => editorRef.current?.tableCommand('addRowAfter') },
                      { label: 'Add row above', onSelect: () => editorRef.current?.tableCommand('addRowBefore') },
                      { label: 'Add column right', onSelect: () => editorRef.current?.tableCommand('addColumnAfter') },
                      { label: 'Add column left', onSelect: () => editorRef.current?.tableCommand('addColumnBefore') },
                      { label: 'Delete row', onSelect: () => editorRef.current?.tableCommand('deleteRow') },
                      { label: 'Delete column', onSelect: () => editorRef.current?.tableCommand('deleteColumn') },
                      { label: 'Delete table', danger: true, onSelect: () => editorRef.current?.tableCommand('deleteTable') },
                      'sep',
                    ] as const)
                  : ([{ label: 'Insert table', onSelect: () => editorRef.current?.tableCommand('insertTable') }, 'sep'] as const)),
                { label: 'Ask Muse about selection', disabled: !sel, onSelect: () => setMuseSel({ text: sel, source: 'editor' }) },
                { label: 'Copy selection', disabled: !sel, onSelect: () => void navigator.clipboard.writeText(sel) },
                { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?d=${docId}`) },
              ])
            }}
          >
            <RichEditor
              key={`${docId}-${seed}`}
              ref={editorRef}
              value={doc.body}
              docSearch={docSearch}
              slash
              prose
              autosave
              onSave={() => void saveBody()}
              placeholder={doc.kind === 'agent' ? 'OKF-structured knowledge for agents' : 'Write'}
              fill
              className="min-w-0 flex-1"
            />
            <MuseBar
              context={`Knowledge document “${title || doc.title}”${folderName ? ` in the “${folderName}” space` : ''}.`}
              currentText={() => editorRef.current?.getMarkdown() ?? doc.body}
              selection={museSel?.text ?? null}
              onClearSelection={() => setMuseSel(null)}
              surgical={!!museSel}
              onAccept={async (md) => {
                await save({ title, body: md })
                setSeed((v) => v + 1)
                setMuseSel(null)
              }}
              onAcceptSelection={async (replacement) => {
                if (!museSel) return
                if (museSel.source === 'editor') {
                  editorRef.current?.replaceSelection(replacement)
                  await saveBody()
                } else {
                  const body = editorRef.current?.getMarkdown() ?? doc.body
                  await save({ title, body: body.replace(museSel.text, replacement) })
                  setSeed((v) => v + 1)
                }
                setMuseSel(null)
              }}
            />
          </div>
        ) : (
          // Read mode: rendered markdown with the identical measure/typography as
          // the editor (both use .re-prose), so switching modes doesn't reflow.
          <div className="flex min-w-0 flex-1 flex-col">
          <div
            ref={readRef}
            className="re-prose relative min-w-0 flex-1 overflow-y-auto"
            onContextMenu={(e) => {
              const sel = window.getSelection()?.toString().trim() ?? ''
              openDocMenu(e, [
                { label: 'Copy text', disabled: !sel && !doc.body, onSelect: () => void navigator.clipboard.writeText(sel || doc.body) },
                { label: 'Copy link', onSelect: () => copyAppLink(`/knowledge?d=${docId}`) },
                ...(sel
                  ? [
                      {
                        label: 'Comment on selection',
                        onSelect: () => {
                          setPendingQuote(sel.slice(0, 500))
                          setShowComments(true)
                        },
                      },
                      { label: 'Ask Muse about selection', onSelect: () => setMuseSel({ text: sel, source: 'read' }) },
                    ]
                  : []),
                { label: showComments ? 'Hide comments' : 'Comments', onSelect: () => setShowComments((v) => !v) },
              ])
            }}
            onClick={(e) => {
              const mark = (e.target as HTMLElement).closest?.('[data-kb-mark]') as HTMLElement | null
              if (mark?.dataset.kbMark) {
                setShowComments(true)
                setFocusThread(mark.dataset.kbMark)
              }
            }}
            onMouseUp={() => {
              const sel = window.getSelection()
              const text = sel?.toString().trim() ?? ''
              if (!text || text.length > 500 || !sel || sel.rangeCount === 0) {
                setSelPop(null)
                return
              }
              const rect = sel.getRangeAt(0).getBoundingClientRect()
              const host = bodyRef.current?.getBoundingClientRect()
              if (!host) return
              setSelPop({ x: rect.left - host.left + rect.width / 2, y: rect.top - host.top - 8, quote: text })
            }}
          >
            {selPop && (
              <div
                style={{ left: selPop.x, top: Math.max(selPop.y, 4) }}
                className="absolute z-20 flex -translate-x-1/2 -translate-y-full overflow-hidden rounded-lg border border-line bg-card text-xs text-fg shadow-lg"
              >
                <button
                  type="button"
                  className="px-2 py-1 transition-colors hover:bg-sidebar"
                  onMouseDown={(e) => {
                    e.preventDefault()
                    setPendingQuote(selPop.quote)
                    setShowComments(true)
                    setSelPop(null)
                    window.getSelection()?.removeAllRanges()
                  }}
                >
                  <MessageSquareText size={12} className="mr-1 inline" /> Comment
                </button>
                <span className="my-1 border-l border-line-subtle" />
                <button
                  type="button"
                  className="px-2 py-1 text-accent transition-colors hover:bg-sidebar"
                  onMouseDown={(e) => {
                    e.preventDefault()
                    setMuseSel({ text: selPop.quote, source: 'read' })
                    setSelPop(null)
                    window.getSelection()?.removeAllRanges()
                  }}
                >
                  <Sparkles size={12} className="mr-1 inline" /> Muse
                </button>
              </div>
            )}
            {doc.body.trim() ? (
              <Markdown className="tiptap">{doc.body}</Markdown>
            ) : (
              <div className="mx-auto max-w-[46rem] px-6 py-8">
                <button type="button" onClick={() => setMode('edit')} className="text-sm text-muted hover:text-fg">
                  This document is empty. Click to start writing.
                </button>
              </div>
            )}

            {/* Backlinks — docs that reference this one. */}
            {backlinksLoading && (
              <div aria-hidden className="mx-auto max-w-[46rem] px-6 pb-10">
                <div className="border-t border-line-subtle pt-4">
                  <Skeleton className="mb-3 h-2.5 w-24 rounded-full" />
                  <SkeletonRows rows={2} />
                </div>
              </div>
            )}
            {backlinksList.notice && (
              <div className="mx-auto max-w-[46rem] px-6 pb-4">{backlinksList.notice}</div>
            )}
            {!backlinksLoading && backlinks.length > 0 && (
              <div className="mx-auto max-w-[46rem] px-6 pb-10">
                <div className="border-t border-line-subtle pt-4">
                  <div className="mb-2 flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-muted">
                    <Link2 size={12} /> Linked from
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {backlinks.map((b) => (
                      <button
                        key={b.id}
                        type="button"
                        onClick={() => onSelect(b.id)}
                        className="flex items-center gap-1.5 rounded-lg border border-line-subtle px-2 py-1 text-xs text-muted hover:text-fg"
                      >
                        <span>{b.icon ?? '📄'}</span>
                        <span className="max-w-[16rem] truncate">{b.title}</span>
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            )}

            <ArtifactAttachments docId={docId} />
          </div>
          <MuseBar
            context={`Knowledge document “${title || doc.title}”${folderName ? ` in the “${folderName}” space` : ''}.`}
            currentText={() => doc.body}
            selection={museSel?.text ?? null}
            onClearSelection={() => setMuseSel(null)}
            surgical={!!museSel && doc.body.includes(museSel.text)}
            onAccept={async (md) => {
              await save({ title, body: md })
              setMuseSel(null)
            }}
            onAcceptSelection={async (replacement) => {
              if (!museSel) return
              await save({ title, body: doc.body.replace(museSel.text, replacement) })
              setMuseSel(null)
            }}
          />
          </div>
        )}

        {showComments && (
          <CommentsPanel
            docId={docId}
            comments={comments}
            loadFailed={commentsQuery.isError && commentsQuery.data === undefined}
            loadError={commentsQuery.error}
            onRetryLoad={() => void commentsQuery.refetch()}
            meId={me?.id ?? null}
            docOwnerId={doc.ownerUserId}
            pendingQuote={pendingQuote}
            onQuoteConsumed={() => setPendingQuote(null)}
            focusId={focusThread}
            onFocusConsumed={() => setFocusThread(null)}
            onClose={() => setShowComments(false)}
          />
        )}
        {showToc && (
          <TocPanel
            headings={headings}
            onJump={scrollToHeading}
            onClose={() => setShowToc(false)}
            emptyText="No headings yet. Add one (H1–H3) and it shows up here."
          />
        )}
        {showHistory && (
          <div className="w-64 shrink-0 overflow-y-auto border-l border-line-subtle p-3">
            <HistoryRail
              id={docId}
              onRestore={async (content) => {
                // Snapshots are stored as `# Title\n\n<body>`; split them back out.
                const m = /^#\s+(.*)\n+([\s\S]*)$/.exec(content)
                const t = m ? m[1]!.trim() : title
                const b = m ? m[2]! : content
                setTitle(t)
                await save({ title: t, body: b })
                setSeed((n) => n + 1)
              }}
            />
          </div>
        )}
      </div>

      {okfOpen && doc.okf && (
        <Modal open onClose={() => setOkfOpen(false)} title="OKF — agent-facing summary">
          <div className="space-y-3">
            <p className="font-sans text-xs text-muted">
              What agents read instead of the full document: an Open Knowledge Format concept the Librarian maintains from the
              promoted content. It refreshes automatically when this document changes.
            </p>
            <pre className="max-h-96 overflow-y-auto whitespace-pre-wrap rounded-xl bg-card/60 p-3 text-xs leading-relaxed text-fg">{doc.okf}</pre>
            {isOwner && (
              <div className="flex justify-end border-t border-line-subtle pt-3">
                <Button size="sm" variant="outline" onClick={() => void save({ regenerateOkf: true } as never)}>
                  Regenerate now
                </Button>
              </div>
            )}
          </div>
        </Modal>
      )}
      {docMenu}
      <PermissionsModal
        open={shareOpen}
        onClose={() => setShareOpen(false)}
        kind="docs"
        id={docId}
        label={doc.title}
        visibility={doc.visibility}
        editPolicy={doc.editPolicy}
        publicSlug={doc.publicSlug}
        canManage={isOwner}
        inheritable
        inherited={doc.permsInherited}
        folderName={folderName}
        onSave={(patch) => save(patch)}
      />
    </div>
  )
}

const VisibilityIcon = ({ v }: { v: 'private' | 'org' | 'public' }) =>
  v === 'public' ? <Globe size={12} /> : v === 'private' ? <Lock size={12} /> : <Users size={12} />

// Attach any artifact to a KB doc (the "attach an artifact to anything" spec).
function ArtifactAttachments({ docId }: { docId: string }) {
  const qc = useQueryClient()
  // Two defaults, two different lies: an empty `attached` says this document
  // has nothing attached to it, and an empty `all` says there is nothing in
  // the org to attach.
  const attachedList = listQuery(useTargetArtifacts('kb-doc', docId), { title: 'Could not load this document’s attachments', variant: 'inline' })
  const allList = listQuery(useArtifacts(), { title: 'Could not load your artifacts', variant: 'inline' })
  const attached = attachedList.rows
  const all = allList.rows
  const loading = attachedList.pending || allList.pending
  const attachedIds = new Set(attached.map((a) => a.id))
  const options = all.filter((a) => !attachedIds.has(a.id)).map((a) => ({ value: a.id, label: a.title, sub: a.kind }))
  const refresh = () => qc.invalidateQueries({ queryKey: ['artifacts-for', 'kb-doc', docId] })
  if (loading) {
    return (
      <div aria-hidden className="mx-auto max-w-[46rem] px-6 pb-10">
        <div className="border-t border-line-subtle pt-4">
          <div className="mb-2 flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-muted">
            <Paperclip size={12} /> Attachments
          </div>
          <div className="flex flex-wrap gap-2">
            <Skeleton className="h-7 w-36" />
            <Skeleton className="h-7 w-28" delay={0.12} />
          </div>
        </div>
      </div>
    )
  }
  return (
    <div className="mx-auto max-w-[46rem] px-6 pb-10">
      <div className="border-t border-line-subtle pt-4">
        <div className="mb-2 flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-muted">
          <Paperclip size={12} /> Attachments
        </div>
        {attachedList.notice}
        {allList.notice}
        {attached.length > 0 && (
          <div className="mb-2 flex flex-wrap gap-2">
            {attached.map((a) => (
              <span key={a.id} className="flex items-center gap-1.5 rounded-lg border border-line-subtle px-2 py-1 text-xs text-muted">
                <span>{a.icon ?? '◆'}</span>
                <span className="max-w-[14rem] truncate text-fg">{a.title}</span>
                <span className="text-[9px] uppercase tracking-wide">{a.kind}</span>
                <CloseButton
                  size={11}
                  label="Detach"
                  onClick={async () => { await detachArtifact(a.id, 'kb-doc', docId); await refresh() }}
                  className="p-0 hover:bg-transparent hover:text-[color:var(--theme-danger)]"
                />
              </span>
            ))}
          </div>
        )}
        <Combobox
          options={options}
          selected={[]}
          onChange={async (v) => { if (v[0]) { await attachArtifact(v[0], 'kb-doc', docId); await refresh() } }}
          placeholder="Attach an artifact"
          size="sm"
          className="max-w-xs"
        />
      </div>
    </div>
  )
}

interface Rev {
  id: string
  createdBy: string | null
  createdAt: string
  size: number
}
function HistoryRail({ kind = 'kb-doc', id, onRestore }: { kind?: 'kb-doc' | 'kb-space'; id: string; onRestore: (content: string) => Promise<void> }) {
  const [revs, setRevs] = useState<Rev[]>([])
  const [loading, setLoading] = useState(true)
  // `r.ok ? r.json() : { revisions: [] }` told the owner of a document that it
  // has NO VERSION HISTORY — on the one rail whose entire purpose is restoring
  // a previous version. If the read fails the honest answer is "we could not
  // read it", not "there is nothing to go back to".
  const [error, setError] = useState<unknown>(null)
  const [reload, setReload] = useState(0)
  const [preview, setPreview] = useState<{ rev: Rev; content: string } | null>(null)
  const [restoring, setRestoring] = useState(false)
  useEffect(() => {
    // Both kb-doc and kb-space history key on the item id (like memory).
    let live = true
    setLoading(true)
    setError(null)
    getJson<{ revisions?: Rev[] }>(`/api/history?kind=${kind}&id=${id}`)
      .then((d) => {
        if (!live) return
        setRevs(d.revisions ?? [])
      })
      .catch((e: unknown) => {
        if (!live) return
        setRevs([])
        setError(e)
      })
      .finally(() => live && setLoading(false))
    return () => {
      live = false
    }
  }, [kind, id, reload])

  const open = async (rev: Rev) => {
    const r = await fetch(`/api/history?kind=${kind}&id=${id}&rev=${rev.id}`)
    if (!r.ok) return
    const { content } = (await r.json()) as { content: string }
    setPreview({ rev, content })
  }

  return (
    <div>
      <div className="mb-2 text-[11px] uppercase tracking-wide text-muted">History</div>
      {loading ? (
        <SkeletonRows rows={4} className="px-2 py-1" />
      ) : error ? (
        <QueryError
          variant="inline"
          title="Could not load this document’s history"
          error={error}
          onRetry={() => setReload((n) => n + 1)}
        />
      ) : revs.length === 0 ? (
        <EmptyState variant="inline" title="No saved revisions yet." />
      ) : (
        revs.map((r, i) => (
          <button
            key={r.id}
            type="button"
            onClick={() => void open(r)}
            className="block w-full rounded-md px-2 py-1.5 text-left text-xs hover:bg-card"
          >
            <div className="text-fg">{i === 0 ? 'Latest' : relativeTime(r.createdAt)}</div>
            <div className="text-[11px] text-muted">{r.createdBy ?? 'unknown'} · {r.size} chars</div>
          </button>
        ))
      )}

      <Modal
        open={!!preview}
        onClose={() => setPreview(null)}
        title={preview ? `Revision · ${relativeTime(preview.rev.createdAt)}` : 'Revision'}
        width="max-w-3xl"
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="outline" size="sm" onClick={() => setPreview(null)}>
              Close
            </Button>
            <Button
              size="sm"
              disabled={restoring}
              onClick={async () => {
                if (!preview) return
                setRestoring(true)
                try {
                  await onRestore(preview.content)
                  setPreview(null)
                } finally {
                  setRestoring(false)
                }
              }}
            >
              {restoring ? 'Restoring' : 'Restore this version'}
            </Button>
          </div>
        }
      >
        <div className="max-h-[60vh] overflow-y-auto">
          {preview && <Markdown>{preview.content}</Markdown>}
        </div>
      </Modal>
    </div>
  )
}

/** Notion-shaped comment threads: roots (optionally anchored to a quote),
 *  replies, resolve. Lives as a side panel beside the doc body. */
function CommentsPanel({
  docId,
  comments,
  loadFailed,
  loadError,
  onRetryLoad,
  meId,
  docOwnerId,
  pendingQuote,
  onQuoteConsumed,
  focusId,
  onFocusConsumed,
  onClose,
}: {
  docId: string
  comments: KbComment[]
  /** The thread read FAILED — the empty list below is ignorance, not silence. */
  loadFailed: boolean
  loadError: unknown
  onRetryLoad: () => void
  meId: string | null
  docOwnerId: string | null
  pendingQuote: string | null
  onQuoteConsumed: () => void
  focusId?: string | null
  onFocusConsumed?: () => void
  onClose: () => void
}) {
  const qc = useQueryClient()
  const [draft, setDraft] = useState('')
  const [replyTo, setReplyTo] = useState<string | null>(null)
  const [replyDraft, setReplyDraft] = useState('')
  const [showResolved, setShowResolved] = useState(false)
  const refresh = () => qc.invalidateQueries({ queryKey: ['kb-comments', docId] })

  const post = async (content: string, parentId: string | null, quote: string | null) => {
    if (!content.trim()) return
    await fetch(`/api/kb/docs/${docId}/comments`, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content: content.trim(), parentId, quote }),
    })
    await refresh()
  }
  const setResolved = async (id: string, resolved: boolean) => {
    await fetch(`/api/kb/comments/${id}`, {
      method: 'PATCH',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ resolved }),
    })
    await refresh()
  }
  const remove = async (id: string) => {
    await fetch(`/api/kb/comments/${id}`, { method: 'DELETE', credentials: 'same-origin' })
    await refresh()
  }

  // A mark click lands here: scroll its thread into view and flash it.
  useEffect(() => {
    if (!focusId) return
    const el = document.querySelector(`[data-kb-thread="${focusId}"]`)
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'center' })
      el.classList.add('kb-comment-flash')
      setTimeout(() => el.classList.remove('kb-comment-flash'), 1300)
    }
    onFocusConsumed?.()
  }, [focusId, onFocusConsumed])

  const roots = comments.filter((c) => !c.parentId)
  const open = roots.filter((c) => !c.resolved)
  const resolved = roots.filter((c) => c.resolved)
  const repliesOf = (id: string) => comments.filter((c) => c.parentId === id)

  const Thread = ({ root }: { root: KbComment }) => (
    <div data-kb-thread={root.id} className={cn('space-y-2 rounded-xl border border-line-subtle/70 p-2.5', root.resolved && 'opacity-60')}>
      {root.quote && (
        <div className="border-l-2 border-accent/50 pl-2 font-sans text-[11px] italic text-muted line-clamp-2">“{root.quote}”</div>
      )}
      <CommentBody c={root} meId={meId} onDelete={() => void remove(root.id)} />
      {repliesOf(root.id).map((r) => (
        <div key={r.id} className="flex gap-1.5 pl-3">
          <CornerDownRight size={11} className="mt-1 shrink-0 text-muted/60" />
          <div className="min-w-0 flex-1">
            <CommentBody c={r} meId={meId} onDelete={() => void remove(r.id)} />
          </div>
        </div>
      ))}
      <div className="flex items-center gap-2 pl-3">
        {replyTo === root.id ? (
          <Textarea
            autoFocus
            autoGrow
            rows={1}
            value={replyDraft}
            onChange={(e) => setReplyDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                void post(replyDraft, root.id, null).then(() => {
                  setReplyDraft('')
                  setReplyTo(null)
                })
              } else if (e.key === 'Escape') {
                setReplyTo(null)
              }
            }}
            placeholder="Reply"
            className="min-h-0 flex-1 text-xs"
          />
        ) : (
          <button type="button" onClick={() => setReplyTo(root.id)} className="text-[11px] text-muted hover:text-accent">
            Reply
          </button>
        )}
        {(root.authorUserId === meId || docOwnerId === meId) && (
          <button
            type="button"
            onClick={() => void setResolved(root.id, !root.resolved)}
            className="ml-auto flex items-center gap-1 text-[11px] text-muted hover:text-[color:var(--theme-success)]"
            title={root.resolved ? 'Reopen this thread' : 'Resolve this thread'}
          >
            <CheckCircle2 size={12} /> {root.resolved ? 'Reopen' : 'Resolve'}
          </button>
        )}
      </div>
    </div>
  )

  return (
    <aside className="flex w-80 shrink-0 flex-col border-l border-line-subtle">
      <div className="flex items-center gap-2 border-b border-line-subtle px-3 py-2">
        <MessageSquareText size={13} className="text-muted" />
        <span className="text-sm font-semibold text-fg">Comments</span>
        <span className="flex-1" />
        {resolved.length > 0 && (
          <button type="button" onClick={() => setShowResolved((v) => !v)} className="text-[11px] text-muted hover:text-fg">
            {showResolved ? 'Hide resolved' : `Resolved (${resolved.length})`}
          </button>
        )}
        <CloseButton onClick={onClose} size={13} className="h-6 w-6 rounded p-0" />
      </div>
      <div className="min-h-0 flex-1 space-y-2.5 overflow-y-auto p-3">
        {loadFailed ? (
          <QueryError variant="inline" error={loadError} title="Could not load comments" onRetry={onRetryLoad} />
        ) : (
          open.length === 0 &&
          !showResolved && <div className="font-sans text-xs text-muted">No open threads. Select text in the doc to comment on it.</div>
        )}
        {open.map((c) => (
          <Thread key={c.id} root={c} />
        ))}
        {showResolved && resolved.map((c) => <Thread key={c.id} root={c} />)}
      </div>
      <div className="border-t border-line-subtle p-3">
        {pendingQuote && (
          <div className="mb-1.5 flex items-start gap-1.5 border-l-2 border-accent/50 pl-2 font-sans text-[11px] italic text-muted">
            <span className="min-w-0 flex-1 line-clamp-2">“{pendingQuote}”</span>
            <CloseButton onClick={onQuoteConsumed} size={11} className="shrink-0 p-0 hover:bg-transparent" />
          </div>
        )}
        <Textarea
          autoGrow
          rows={1}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              void post(draft, null, pendingQuote).then(() => {
                setDraft('')
                onQuoteConsumed()
              })
            }
          }}
          placeholder={pendingQuote ? 'Comment on the selection' : 'Start a thread'}
          className="text-sm"
        />
      </div>
    </aside>
  )
}

function CommentBody({ c, meId, onDelete }: { c: KbComment; meId: string | null; onDelete: () => void }) {
  return (
    <div className="group/comment">
      <div className="flex items-baseline gap-1.5">
        <Avatar name={c.author} className="h-4 w-4 self-center text-[8px]" />
        <span className="text-xs font-medium text-fg">{c.author}</span>
        <span className="text-[10px] text-muted">{relativeTime(c.createdAt)}</span>
        {c.authorUserId === meId && (
          <button
            type="button"
            onClick={onDelete}
            className="ml-auto text-muted opacity-0 transition-opacity hover:text-[color:var(--theme-danger)] group-hover/comment:opacity-100"
            title="Delete your comment"
          >
            <Trash2 size={11} />
          </button>
        )}
      </div>
      <div className="whitespace-pre-wrap pl-5 font-sans text-xs leading-relaxed text-fg">{c.content}</div>
    </div>
  )
}

/** Muse as the knowledge worker — ALWAYS present under the doc (read and
 *  edit): describe a change and it drafts from the current document. With a
 *  SELECTION in scope it works surgically — the proposal is a replacement for
 *  just that passage, applied in place on accept. Refinements keep short chat
 *  memory. */
function MuseBar({
  context,
  currentText,
  selection,
  surgical,
  onClearSelection,
  onAccept,
  onAcceptSelection,
}: {
  context: string
  currentText: () => string
  selection?: string | null
  /** True when the selection can be replaced in place. */
  surgical?: boolean
  onClearSelection?: () => void
  onAccept: (markdown: string) => Promise<void>
  onAcceptSelection?: (replacement: string) => Promise<void>
}) {
  const [instruction, setInstruction] = useState('')
  const [proposal, setProposal] = useState<string | null>(null)
  const [proposalScope, setProposalScope] = useState<'doc' | 'selection'>('doc')
  const [generating, setGenerating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [chat, setChat] = useState<Array<{ role: 'user' | 'assistant'; content: string }>>([])
  const abortRef = useRef<AbortController | null>(null)
  useEffect(() => () => abortRef.current?.abort(), [])

  const generate = async () => {
    const instr = instruction.trim()
    if (!instr || generating) return
    setGenerating(true)
    setError(null)
    setProposal('')
    const scope = selection && surgical ? 'selection' : 'doc'
    setProposalScope(scope)
    const ac = new AbortController()
    abortRef.current = ac
    try {
      const current =
        proposal !== null && proposalScope === scope
          ? proposal
          : scope === 'selection'
            ? selection!
            : currentText()
      const ctx =
        scope === 'selection'
          ? `${context} You are editing ONLY this passage of the document — reply with the replacement passage alone, no commentary.`
          : selection
            ? `${context} Focus especially on this passage: «${selection.slice(0, 400)}»`
            : context
      const full = await streamMuse(
        { kind: 'document', context: ctx, current, instruction: instr, chat },
        (piece) => setProposal((prev) => (prev ?? '') + piece),
        ac.signal,
      )
      setChat((c) => [...c.slice(-10), { role: 'user', content: instr }, { role: 'assistant', content: full }])
      setInstruction('')
    } catch (e) {
      if (!ac.signal.aborted) {
        setError((e as Error).message)
        setProposal(null)
      }
    } finally {
      setGenerating(false)
    }
  }

  const accept = () => {
    if (proposal === null) return
    const md = proposal
    setProposal(null)
    if (proposalScope === 'selection' && onAcceptSelection) void onAcceptSelection(md.trim())
    else void onAccept(md)
  }

  return (
    <div className="space-y-2 border-t border-line-subtle px-4 py-2.5">
      {selection && (
        <div className="flex items-start gap-1.5 border-l-2 border-accent/50 pl-2 font-sans text-[11px] italic text-muted">
          <span className="min-w-0 flex-1 line-clamp-2">
            {surgical ? 'editing selection: ' : 'focused on: '}“{selection}”
          </span>
          <CloseButton
            onClick={() => onClearSelection?.()}
            size={11}
            label="Back to whole-document edits"
            className="shrink-0 p-0 hover:bg-transparent"
          />
        </div>
      )}
      {proposal !== null && (
        <div className="max-h-56 overflow-y-auto rounded-xl border border-accent/30 bg-card/40 p-3">
          <Markdown className="tiptap text-sm">{proposal}</Markdown>
        </div>
      )}
      {error && <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>{error}</div>}
      <div className="flex items-end gap-2">
        <Sparkles size={14} className="mb-2.5 shrink-0 text-accent" />
        <Textarea
          autoGrow
          rows={1}
          value={instruction}
          onChange={(e) => setInstruction(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              void generate()
            }
          }}
          placeholder={
            proposal !== null
              ? 'Refine the proposal, e.g. “tighter, and add a checklist”'
              : selection
                ? surgical
                  ? 'How should this passage change?'
                  : 'What about this passage? (it will guide a whole-doc draft)'
                : 'Edit with Muse — describe the change, or select text for inline edits'
          }
          className="max-h-32 text-sm"
        />
        <Button size="sm" className="shrink-0" onClick={() => void generate()} disabled={generating || !instruction.trim()}>
          {generating ? 'Drafting' : proposal !== null ? 'Refine' : 'Draft'}
        </Button>
        {proposal !== null && !generating && (
          <>
            <Button size="sm" variant="outline" className="shrink-0" onClick={accept}>
              {proposalScope === 'selection' ? 'Replace passage' : 'Accept'}
            </Button>
            <Button size="sm" variant="ghost" className="shrink-0" onClick={() => setProposal(null)}>
              Discard
            </Button>
          </>
        )}
      </div>
    </div>
  )
}
