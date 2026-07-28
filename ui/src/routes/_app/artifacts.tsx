import { createFileRoute } from '@tanstack/react-router'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { FileText, Table, Globe2, Paperclip, Trash2, History, Maximize2, Minimize2, MoreHorizontal, Plus, Star, ChevronRight, Folder, FolderPlus, X, Upload, ExternalLink, DownloadCloud, Search, type LucideIcon } from 'lucide-react'
import { Button, buttonClasses } from '@/components/ui/button'
import { confirm, alert } from '@/components/ui/confirm'
import { Input } from '@/components/ui/input'
import { inlineEditKeys } from '@/components/ui/control'
import { Textarea } from '@/components/ui/textarea'
import { EmptyState } from '@/components/ui/empty-state'
import { EmojiPicker } from '@/components/ui/emoji-picker'
import { Markdown } from '@/components/ui/markdown'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { useContextMenu, copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu'
import { PermissionsModal } from '@/components/kb/permissions-modal'
import { BrainRoutingSelect } from '@/components/kb/brain-select'
import { IconButton } from '@/components/ui/icon-button'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import { useSession } from '@/lib/session'
import { createArtifact, createFolder, deleteArtifact, deleteFolder, saveArtifact, updateFolder, uploadFile, useArtifact, useArtifacts, useFolders, type Artifact, type ArtifactFolder, type ArtifactKind } from '@/lib/artifacts'

export const Route = createFileRoute('/_app/artifacts')({
  // ?a=<artifactId> deep-links an artifact — the URL IS the selection.
  validateSearch: (search: Record<string, unknown>): { a?: string } =>
    typeof search.a === 'string' && search.a ? { a: search.a } : {},
  component: ArtifactsPage,
})

const KIND_ICON: Record<ArtifactKind, LucideIcon> = { doc: FileText, sheet: Table, microsite: Globe2, file: Paperclip }

// Artifacts — versioned work products with their own hosting + sharing. This
// foundation covers the doc kind (markdown); sheets, microsites, and files, plus
// cloud-storage connectors and the "make official → knowledgebase" pipeline, are
// tracked follow-ups.
const NEW_KINDS: { kind: ArtifactKind; label: string; icon: LucideIcon }[] = [
  { kind: 'doc', label: 'Document', icon: FileText },
  { kind: 'microsite', label: 'Microsite', icon: Globe2 },
  { kind: 'file', label: 'File upload', icon: Paperclip },
]

type Drag = { kind: 'artifact' | 'folder'; id: string } | null

function ArtifactsPage() {
  const qc = useQueryClient()
  const { data: artifacts = [], isLoading: artifactsLoading } = useArtifacts()
  const { data: folders = [], isLoading: foldersLoading } = useFolders()
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const activeId = search.a ?? null
  const setActiveId = (id: string | null) => void navigate({ search: id ? { a: id } : {} })
  const [newOpen, setNewOpen] = useState(false)
  const [importOpen, setImportOpen] = useState(false)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [drag, setDrag] = useState<Drag>(null)

  const byFolder = useMemo(() => {
    const m = new Map<string | null, Artifact[]>()
    for (const a of artifacts) {
      const k = a.folderId ?? null
      if (!m.has(k)) m.set(k, [])
      m.get(k)!.push(a)
    }
    return m
  }, [artifacts])
  const foldersByParent = useMemo(() => {
    const m = new Map<string | null, ArtifactFolder[]>()
    for (const f of folders) {
      const k = f.parentId ?? null
      if (!m.has(k)) m.set(k, [])
      m.get(k)!.push(f)
    }
    for (const list of m.values()) list.sort((a, b) => a.name.localeCompare(b.name))
    return m
  }, [folders])

  const refresh = () => Promise.all([qc.invalidateQueries({ queryKey: ['artifacts'] }), qc.invalidateQueries({ queryKey: ['artifact-folders'] })])
  const create = async (kind: ArtifactKind) => {
    setNewOpen(false)
    const { artifact } = await createArtifact({ kind, title: 'Untitled' })
    await qc.invalidateQueries({ queryKey: ['artifacts'] })
    if (artifact) setActiveId(artifact.id)
  }
  const newFolder = async (parentId: string | null = null) => {
    const { folder } = await createFolder('New folder', parentId)
    await qc.invalidateQueries({ queryKey: ['artifact-folders'] })
    if (folder) {
      setExpanded((s) => {
        const n = new Set(s).add(folder.id)
        if (parentId) n.add(parentId) // reveal the new subfolder
        return n
      })
    }
  }

  // Right-click menus — shortcuts to actions the tree and editor already offer.
  const { openMenu, menu } = useContextMenu()
  const artifactMenu = (a: Artifact): ContextMenuEntry[] => {
    const items: ContextMenuEntry[] = [
      { label: 'Open', onSelect: () => setActiveId(a.id) },
      { label: 'Copy link', onSelect: () => copyAppLink(`/artifacts?a=${a.id}`) },
    ]
    const slug = a.publicSlug
    if (slug) items.push({ label: 'Copy public link', onSelect: () => copyAppLink(`/a/${slug}`) })
    items.push('sep', {
      label: 'Delete artifact',
      danger: true,
      onSelect: async () => {
        // Same confirm + deleteArtifact flow as the editor's kebab menu.
        if (!(await confirm({ title: 'Delete artifact', message: `Delete "${a.title}"?`, confirmLabel: 'Delete', danger: true }))) return
        await deleteArtifact(a.id)
        await qc.invalidateQueries({ queryKey: ['artifacts'] })
        if (activeId === a.id) setActiveId(null)
      },
    })
    return items
  }
  // Drop the dragged item into a folder (or root when folderId is null).
  const drop = async (folderId: string | null) => {
    if (!drag) return
    if (drag.kind === 'artifact') await saveArtifact(drag.id, { folderId })
    else await updateFolder(drag.id, { parentId: folderId })
    setDrag(null)
    await refresh()
  }

  const rootArtifacts = byFolder.get(null) ?? []
  const rootFolders = foldersByParent.get(null) ?? []

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex h-full w-72 shrink-0 flex-col border-r border-line-subtle bg-sidebar font-sans">
        <div className="relative flex h-12 shrink-0 items-center gap-1.5 border-b border-line-subtle px-4">
          <span className="min-w-0 flex-1 truncate text-sm font-semibold text-fg">Artifacts</span>
          <div className="flex items-center gap-0.5">
            <IconButton size="sm" title="Import from Google Drive" onClick={() => setImportOpen(true)}>
              <DownloadCloud size={15} />
            </IconButton>
            <IconButton size="sm" title="New folder" onClick={() => void newFolder()}>
              <FolderPlus size={15} />
            </IconButton>
            <IconButton size="sm" title="New artifact" onClick={() => setNewOpen((v) => !v)} active={newOpen}>
              <Plus size={15} />
            </IconButton>
          </div>
          {newOpen && (
            <div className="absolute right-3 top-full z-30 mt-1 w-44 rounded-xl border border-line bg-card p-1 shadow-lg" onMouseLeave={() => setNewOpen(false)}>
              {NEW_KINDS.map(({ kind, label, icon: Icon }) => (
                <button key={kind} type="button" onClick={() => void create(kind)} className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-xs text-fg hover:bg-sidebar">
                  <Icon size={13} /> {label}
                </button>
              ))}
            </div>
          )}
        </div>
        <div
          className="min-h-0 flex-1 overflow-y-auto p-2"
          onDragOver={(e) => e.preventDefault()}
          onDrop={(e) => { e.preventDefault(); void drop(null) }}
        >
          {artifactsLoading || foldersLoading ? (
            // Both queries feed the same tree — reveal it once, fully formed.
            <SkeletonRows rows={6} className="px-2 py-3" />
          ) : folders.length === 0 && artifacts.length === 0 ? (
            <div className="px-2 py-6 text-center text-xs text-muted">No artifacts yet.</div>
          ) : (
            <>
              {rootFolders.map((f) => (
                <FolderNode
                  key={f.id}
                  folder={f}
                  depth={0}
                  foldersByParent={foldersByParent}
                  byFolder={byFolder}
                  expanded={expanded}
                  setExpanded={setExpanded}
                  activeId={activeId}
                  onSelect={setActiveId}
                  drag={drag}
                  setDrag={setDrag}
                  onDrop={drop}
                  onRefresh={refresh}
                  openMenu={openMenu}
                  onArtifactMenu={(e, a) => openMenu(e, artifactMenu(a))}
                  onNewFolder={(parentId) => void newFolder(parentId)}
                />
              ))}
              {rootArtifacts.map((a) => (
                <ArtifactRow key={a.id} artifact={a} depth={0} activeId={activeId} onSelect={setActiveId} setDrag={setDrag} onContextMenu={(e) => openMenu(e, artifactMenu(a))} />
              ))}
            </>
          )}
        </div>
      </aside>
      <main className="min-h-0 min-w-0 flex-1">
        {activeId ? (
          <ArtifactEditor key={activeId} id={activeId} onDeleted={() => setActiveId(null)} />
        ) : (
          <EmptyState icon="◆" title="Artifacts" hint="Create an artifact or a folder. Drag artifacts into folders to organize." />
        )}
      </main>
      {importOpen && (
        <DriveImportModal
          onClose={() => setImportOpen(false)}
          onImported={async (artifactId) => {
            setImportOpen(false)
            await qc.invalidateQueries({ queryKey: ['artifacts'] })
            setActiveId(artifactId)
          }}
        />
      )}
      {menu}
    </div>
  )
}

interface DriveEntry {
  id: string
  name: string
  mimeType: string
  modifiedTime: string | null
  iconLink: string | null
  sizeBytes: number | null
}

// Browse the connected user's Google Drive and pull a file in as an artifact.
function DriveImportModal({ onClose, onImported }: { onClose: () => void; onImported: (artifactId: string) => void }) {
  const [q, setQ] = useState('')
  const [files, setFiles] = useState<DriveEntry[] | null>(null)
  const [state, setState] = useState<'loading' | 'ready' | 'not_connected' | 'reconnect' | 'error'>('loading')
  const [importing, setImporting] = useState<string | null>(null)

  const load = async (query: string) => {
    setState('loading')
    const r = await fetch(`/api/integrations/google/drive/files?q=${encodeURIComponent(query)}`).catch(() => null)
    if (!r) return setState('error')
    if (r.ok) {
      setFiles(((await r.json()) as { files: DriveEntry[] }).files)
      return setState('ready')
    }
    const j = (await r.json().catch(() => null)) as { error?: string } | null
    if (j?.error === 'not_connected') return setState('not_connected')
    if (j?.error === 'reconnect_needed') return setState('reconnect')
    setState('error')
  }
  // Debounced search.
  useEffect(() => {
    const t = setTimeout(() => void load(q), q ? 300 : 0)
    return () => clearTimeout(t)
  }, [q])

  const doImport = async (fileId: string) => {
    setImporting(fileId)
    try {
      const r = await fetch('/api/integrations/google/drive/import', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ fileId }),
      })
      const j = (await r.json().catch(() => null)) as { artifact?: { id: string }; message?: string } | null
      if (r.ok && j?.artifact?.id) onImported(j.artifact.id)
      else await alert({ title: 'Import failed', message: j?.message ?? 'Import failed.' })
    } finally {
      setImporting(null)
    }
  }

  const driveKind = (mime: string) =>
    mime === 'application/vnd.google-apps.document' ? '📄 Doc'
    : mime === 'application/vnd.google-apps.spreadsheet' ? '📊 Sheet'
    : mime === 'application/vnd.google-apps.presentation' ? '📽 Slides'
    : mime.startsWith('image/') ? '🖼 Image'
    : '📎 File'

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 p-4 pt-[10vh]" onClick={onClose}>
      <div className="w-full max-w-lg rounded-2xl border border-line bg-card shadow-xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-3">
          <DownloadCloud size={16} className="text-muted" />
          <span className="text-sm font-semibold text-fg">Import from Google Drive</span>
          <button type="button" onClick={onClose} className="ml-auto rounded p-1 text-muted hover:text-fg"><X size={15} /></button>
        </div>

        {(state === 'not_connected' || state === 'reconnect') ? (
          <div className="p-8 text-center">
            <div className="mb-3 text-sm text-muted">
              {state === 'reconnect'
                ? 'Reconnect Google to grant Drive read access.'
                : 'Connect a Google account to browse your Drive.'}
            </div>
            <a href="/api/integrations/google/connect" className={buttonClasses({ size: 'sm' })}>
              {state === 'reconnect' ? 'Reconnect Google' : 'Connect Google'}
            </a>
          </div>
        ) : (
          <>
            <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-2">
              <Search size={14} className="text-muted" />
              <input
                autoFocus
                value={q}
                onChange={(e) => setQ(e.target.value)}
                placeholder="Search your Drive"
                className="w-full bg-transparent text-sm text-fg outline-none placeholder:text-muted"
              />
            </div>
            <div className="max-h-[50vh] min-h-[12rem] overflow-y-auto p-2">
              {state === 'loading' && (
                // File-row shaped: kind chip + name + time, like the results.
                <div aria-hidden>
                  {['70%', '85%', '55%', '78%', '62%', '88%'].map((w, i) => (
                    <div key={i} className="flex items-center gap-3 px-2 py-2">
                      <Skeleton className="h-3 w-14 shrink-0 rounded-full" delay={i * 0.1} />
                      <div className="min-w-0 flex-1">
                        <div style={{ width: w }}>
                          <Skeleton className="h-3 w-full rounded-full" delay={i * 0.1} />
                        </div>
                      </div>
                      <Skeleton className="h-3 w-10 shrink-0 rounded-full" delay={i * 0.1} />
                    </div>
                  ))}
                </div>
              )}
              {state === 'error' && <div className="p-6 text-center text-xs text-[color:var(--theme-danger)]">Couldn’t reach Google Drive.</div>}
              {state === 'ready' && files && files.length === 0 && <div className="p-6 text-center text-xs text-muted">No files found.</div>}
              {state === 'ready' && files?.map((f) => (
                <button
                  key={f.id}
                  type="button"
                  disabled={!!importing}
                  onClick={() => void doImport(f.id)}
                  className="flex w-full items-center gap-3 rounded-lg px-2 py-2 text-left hover:bg-sidebar disabled:opacity-50"
                >
                  <span className="w-14 shrink-0 text-[11px] text-muted">{driveKind(f.mimeType)}</span>
                  <span className="min-w-0 flex-1 truncate text-sm text-fg">{f.name}</span>
                  <span className="shrink-0 text-[11px] text-muted">
                    {importing === f.id ? 'Importing' : f.modifiedTime ? relativeTime(f.modifiedTime) : ''}
                  </span>
                </button>
              ))}
            </div>
          </>
        )}
      </div>
    </div>
  )
}

function ArtifactRow({ artifact: a, depth, activeId, onSelect, setDrag, onContextMenu }: { artifact: Artifact; depth: number; activeId: string | null; onSelect: (id: string) => void; setDrag: (d: Drag) => void; onContextMenu?: (e: React.MouseEvent) => void }) {
  const Icon = KIND_ICON[a.kind]
  return (
    <button
      type="button"
      draggable
      onDragStart={(e) => { e.stopPropagation(); setDrag({ kind: 'artifact', id: a.id }) }}
      onDragEnd={() => setDrag(null)}
      onClick={() => onSelect(a.id)}
      onContextMenu={onContextMenu}
      className={cn('flex w-full items-center gap-2 rounded-lg py-1.5 pr-2 text-left text-sm', activeId === a.id ? 'bg-card text-fg' : 'text-muted hover:text-fg')}
      style={{ paddingLeft: depth * 14 + 8 }}
    >
      {a.icon ? <span className="text-[15px] leading-none">{a.icon}</span> : <Icon size={14} className="shrink-0" />}
      <span className="min-w-0 flex-1 truncate">{a.title}</span>
      <span className="shrink-0 text-[10px] uppercase tracking-wide text-muted">{a.kind}</span>
    </button>
  )
}

function FolderNode({
  folder, depth, foldersByParent, byFolder, expanded, setExpanded, activeId, onSelect, drag, setDrag, onDrop, onRefresh, openMenu, onArtifactMenu, onNewFolder,
}: {
  folder: ArtifactFolder
  depth: number
  foldersByParent: Map<string | null, ArtifactFolder[]>
  byFolder: Map<string | null, Artifact[]>
  expanded: Set<string>
  setExpanded: React.Dispatch<React.SetStateAction<Set<string>>>
  activeId: string | null
  onSelect: (id: string) => void
  drag: Drag
  setDrag: (d: Drag) => void
  onDrop: (folderId: string | null) => void
  onRefresh: () => Promise<unknown>
  openMenu: (e: React.MouseEvent, items: ContextMenuEntry[]) => void
  onArtifactMenu: (e: React.MouseEvent, a: Artifact) => void
  onNewFolder: (parentId: string) => void
}) {
  const [over, setOver] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const [name, setName] = useState(folder.name)
  useEffect(() => setName(folder.name), [folder.name])
  const isOpen = expanded.has(folder.id)
  const childFolders = foldersByParent.get(folder.id) ?? []
  const childArtifacts = byFolder.get(folder.id) ?? []
  const toggle = () => setExpanded((s) => { const n = new Set(s); n.has(folder.id) ? n.delete(folder.id) : n.add(folder.id); return n })
  // Same confirm + deleteFolder flow as the row's ✕ button.
  const remove = async () => {
    if (await confirm({ title: 'Delete folder', message: `Delete folder "${folder.name}"? Its artifacts move to the top level.`, confirmLabel: 'Delete', danger: true })) {
      await deleteFolder(folder.id)
      await onRefresh()
    }
  }
  const folderMenu = (): ContextMenuEntry[] => [
    { label: 'Rename', onSelect: () => setRenaming(true) },
    { label: 'New folder inside', onSelect: () => onNewFolder(folder.id) },
    'sep',
    { label: 'Delete folder', danger: true, onSelect: () => void remove() },
  ]

  return (
    <div>
      <div
        draggable
        onDragStart={(e) => { e.stopPropagation(); setDrag({ kind: 'folder', id: folder.id }) }}
        onDragEnd={() => setDrag(null)}
        onDragOver={(e) => { e.preventDefault(); e.stopPropagation(); if (drag && !(drag.kind === 'folder' && drag.id === folder.id)) setOver(true) }}
        onDragLeave={() => setOver(false)}
        onDrop={(e) => { e.preventDefault(); e.stopPropagation(); setOver(false); onDrop(folder.id) }}
        onContextMenu={renaming ? undefined : (e) => openMenu(e, folderMenu())}
        className={cn('group flex items-center gap-1 rounded-md py-1 pr-1 text-sm text-muted hover:text-fg', over && 'ring-1 ring-accent/60')}
        style={{ paddingLeft: depth * 14 + 2 }}
      >
        <button type="button" onClick={toggle} className={cn('shrink-0 rounded p-0.5 hover:bg-card', childFolders.length + childArtifacts.length === 0 && 'invisible')}>
          <ChevronRight size={12} className={cn('transition-transform', isOpen && 'rotate-90')} />
        </button>
        {renaming ? (
          <Input
            size="sm"
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            onBlur={async () => { setRenaming(false); if (name.trim() && name !== folder.name) { await updateFolder(folder.id, { name: name.trim() }); await onRefresh() } }}
            onKeyDown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') { setName(folder.name); setRenaming(false) } }}
            className="h-6 flex-1"
          />
        ) : (
          <button type="button" onClick={toggle} onDoubleClick={() => setRenaming(true)} className="flex min-w-0 flex-1 items-center gap-1.5 text-left">
            <span className="shrink-0">{folder.icon ?? <Folder size={13} />}</span>
            <span className="truncate font-medium">{folder.name}</span>
          </button>
        )}
        <button
          type="button"
          title="Delete folder"
          onClick={() => void remove()}
          className="shrink-0 rounded p-0.5 opacity-0 hover:text-[color:var(--theme-danger)] group-hover:opacity-100"
        >
          <X size={12} />
        </button>
      </div>
      {isOpen && (
        <div>
          {childFolders.map((f) => (
            <FolderNode key={f.id} folder={f} depth={depth + 1} foldersByParent={foldersByParent} byFolder={byFolder} expanded={expanded} setExpanded={setExpanded} activeId={activeId} onSelect={onSelect} drag={drag} setDrag={setDrag} onDrop={onDrop} onRefresh={onRefresh} openMenu={openMenu} onArtifactMenu={onArtifactMenu} onNewFolder={onNewFolder} />
          ))}
          {childArtifacts.map((a) => (
            <ArtifactRow key={a.id} artifact={a} depth={depth + 1} activeId={activeId} onSelect={onSelect} setDrag={setDrag} onContextMenu={(e) => onArtifactMenu(e, a)} />
          ))}
        </div>
      )}
    </div>
  )
}

function ArtifactEditor({ id, onDeleted }: { id: string; onDeleted: () => void }) {
  const qc = useQueryClient()
  const { data: me } = useSession()
  const { data: artifact } = useArtifact(id)
  const editorRef = useRef<RichEditorHandle>(null)
  const [title, setTitle] = useState('')
  const [saving, setSaving] = useState(false)
  const [dirty, setDirty] = useState(false)
  const [mode, setMode] = useState<'read' | 'edit'>('read')
  const [emojiOpen, setEmojiOpen] = useState(false)
  const [fullscreen, setFullscreen] = useState(false)
  const [shareOpen, setShareOpen] = useState(false)
  const [showHistory, setShowHistory] = useState(false)
  const [seed, setSeed] = useState(0)
  const [menuOpen, setMenuOpen] = useState(false)
  const [html, setHtml] = useState('') // microsite source
  const htmlTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const initMode = useRef(false)
  useEffect(() => {
    if (artifact) setTitle(artifact.title)
    if (artifact && !initMode.current) {
      initMode.current = true
      setMode(artifact.body.trim() ? 'read' : 'edit')
      if (artifact.kind === 'microsite') setHtml(artifact.body)
    }
  }, [artifact])
  useEffect(() => () => { if (htmlTimer.current) clearTimeout(htmlTimer.current) }, [])
  useEffect(() => {
    if (!fullscreen) return
    const h = (e: KeyboardEvent) => e.key === 'Escape' && setFullscreen(false)
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [fullscreen])

  const isOwner = !!artifact && !!me && (artifact.ownerUserId ? artifact.ownerUserId === me.id : artifact.createdBy === (me.email ?? me.name))

  const save = async (patch: Parameters<typeof saveArtifact>[1]) => {
    setSaving(true)
    try {
      await saveArtifact(id, patch)
      await qc.invalidateQueries({ queryKey: ['artifact', id] })
      await qc.invalidateQueries({ queryKey: ['artifacts'] })
      setDirty(false)
    } finally {
      setSaving(false)
    }
  }
  const saveBody = () => save({ title, body: editorRef.current?.getMarkdown() ?? artifact?.body ?? '' })
  const editHtml = (v: string) => {
    setHtml(v)
    setDirty(true)
    if (htmlTimer.current) clearTimeout(htmlTimer.current)
    htmlTimer.current = setTimeout(() => void save({ body: v }), 700)
  }
  const [uploading, setUploading] = useState(false)
  const onPickFile = async (file: File | null | undefined) => {
    if (!file) return
    setUploading(true)
    try {
      const up = await uploadFile(file)
      await save({ storageRef: up.id, contentType: up.mime, ...(!artifact?.title || artifact.title === 'Untitled' ? { title: up.filename } : {}) })
    } catch {
      /* surfaced by the empty state staying put */
    } finally {
      setUploading(false)
    }
  }

  const [exporting, setExporting] = useState(false)
  const exportToGoogle = async () => {
    setMenuOpen(false)
    setExporting(true)
    try {
      const r = await fetch(`/api/artifacts/${id}/export/google`, { method: 'POST' })
      const j = (await r.json().catch(() => null)) as { file?: { url: string }; error?: string; message?: string } | null
      if (r.ok && j?.file?.url) {
        await qc.invalidateQueries({ queryKey: ['artifact', id] })
        window.open(j.file.url, '_blank', 'noopener')
      } else if (j?.error === 'not_connected') {
        if (await confirm({ title: 'Connect Google', message: 'Connect a Google account to export to Drive. Go to Settings now?', confirmLabel: 'Go to Settings' })) {
          window.location.href = '/settings'
        }
      } else {
        await alert({ title: 'Export failed', message: j?.message ?? 'Export to Google Drive failed.' })
      }
    } finally {
      setExporting(false)
    }
  }
  const googleLabel = artifact?.kind === 'sheet' ? 'Export to Google Sheets' : artifact?.kind === 'file' ? 'Export to Google Drive' : 'Export to Google Docs'

  // Kind is unknown until the fetch lands, so use the doc-page shape (toolbar
  // + centered prose bars) as the default stand-in for every kind.
  if (!artifact) return <ArtifactPageSkeleton />

  return (
    <div className={cn('flex min-h-0 flex-col', fullscreen ? 'fixed inset-0 z-50 bg-surface' : 'h-full')}>
      <div className="flex flex-wrap items-center gap-2 border-b border-line-subtle px-6 py-3">
        <div className="relative shrink-0">
          <button type="button" onClick={() => setEmojiOpen((v) => !v)} className="rounded-lg px-1 text-xl leading-none hover:bg-card" title="Set icon">
            {artifact.icon ?? '📄'}
          </button>
          {emojiOpen && (
            <EmojiPicker onPick={(e) => { void save({ icon: e }); setEmojiOpen(false) }} onClear={() => { void save({ icon: null }); setEmojiOpen(false) }} onClose={() => setEmojiOpen(false)} />
          )}
        </div>
        {mode === 'edit' ? (
          <Input
            value={title}
            onChange={(e) => { setTitle(e.target.value); setDirty(true) }}
            onBlur={() => dirty && void saveBody()}
            onKeyDown={inlineEditKeys(() => artifact && setTitle(artifact.title))}
            className="min-w-0 flex-1 border-0 bg-transparent text-lg font-semibold focus:border-0"
            placeholder="Untitled"
          />
        ) : (
          <h1 className="min-w-0 flex-1 truncate font-sans text-lg font-semibold text-fg">{artifact.title}</h1>
        )}
        <span className="shrink-0 rounded border border-line-subtle px-1.5 text-[10px] uppercase tracking-wide text-muted">{artifact.kind}</span>
        <div className="flex shrink-0 rounded-md border border-line p-0.5">
          {(['read', 'edit'] as const).map((m) => (
            <button key={m} type="button" onClick={() => { if (m === 'read' && mode === 'edit') void saveBody(); setMode(m) }} className={cn('rounded px-2 py-0.5 text-[11px] capitalize transition-colors', mode === m ? 'bg-card text-fg' : 'text-muted hover:text-fg')}>
              {m}
            </button>
          ))}
        </div>
        <Button variant="outline" size="sm" className="shrink-0 capitalize" title="Share &amp; permissions" onClick={() => setShareOpen(true)}>
          {artifact.visibility}
        </Button>
        {isOwner && (
          <Button
            variant={artifact.official ? 'primary' : 'outline'}
            size="sm"
            className="shrink-0"
            title="Official artifacts are mirrored into the knowledgebase and ground the org brain"
            onClick={() => void save({ official: !artifact.official })}
          >
            <Star size={13} className="mr-1" /> {artifact.official ? 'Official' : 'Make official'}
          </Button>
        )}
        <BrainRoutingSelect value={artifact.ragRouting} canEdit={isOwner} onChange={(ragRouting) => void save({ ragRouting })} />
        <Button variant="ghost" size="sm" className="shrink-0" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onClick={() => setFullscreen((v) => !v)}>
          {fullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
        </Button>
        <div className="relative shrink-0">
          <Button variant="ghost" size="sm" title="More" onClick={() => setMenuOpen((v) => !v)}>
            <MoreHorizontal size={14} />
          </Button>
          {menuOpen && (
            <div className="absolute right-0 top-full z-30 mt-1 w-48 rounded-xl border border-line bg-card p-1 shadow-lg" onMouseLeave={() => setMenuOpen(false)}>
              <button type="button" onClick={() => { setShowHistory((v) => !v); setMenuOpen(false) }} className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-xs text-fg hover:bg-sidebar">
                <History size={13} /> {showHistory ? 'Hide history' : 'Version history'}
              </button>
              <button type="button" disabled={exporting} onClick={() => void exportToGoogle()} className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-xs text-fg hover:bg-sidebar disabled:opacity-50">
                <Upload size={13} /> {exporting ? 'Exporting' : googleLabel}
              </button>
              {artifact.googleFileUrl && (
                <a href={artifact.googleFileUrl} target="_blank" rel="noreferrer" onClick={() => setMenuOpen(false)} className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-xs text-fg hover:bg-sidebar">
                  <ExternalLink size={13} /> Open in Google Drive
                </a>
              )}
              <button
                type="button"
                onClick={async () => {
                  setMenuOpen(false)
                  if (!(await confirm({ title: 'Delete artifact', message: `Delete "${artifact.title}"?`, confirmLabel: 'Delete', danger: true }))) return
                  await deleteArtifact(id)
                  await qc.invalidateQueries({ queryKey: ['artifacts'] })
                  onDeleted()
                }}
                className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-xs text-[color:var(--theme-danger)] hover:bg-sidebar"
              >
                <Trash2 size={13} /> Delete artifact
              </button>
            </div>
          )}
        </div>
      </div>

      <div className="flex min-h-0 flex-1">
        {artifact.kind === 'doc' ? (
          mode === 'edit' ? (
            <RichEditor key={`${id}-${seed}`} ref={editorRef} value={artifact.body} slash prose autosave onSave={() => void saveBody()} placeholder="Draft your artifact" fill className="min-w-0 flex-1" />
          ) : (
            <div className="re-prose min-w-0 flex-1 overflow-y-auto">
              {artifact.body.trim() ? (
                <Markdown className="tiptap">{artifact.body}</Markdown>
              ) : (
                <div className="mx-auto max-w-[46rem] px-6 py-8">
                  <button type="button" onClick={() => setMode('edit')} className="text-sm text-muted hover:text-fg">
                    Empty artifact. Click to start.
                  </button>
                </div>
              )}
            </div>
          )
        ) : artifact.kind === 'microsite' ? (
          mode === 'edit' ? (
            <Textarea
              value={html}
              onChange={(e) => editHtml(e.target.value)}
              onBlur={() => dirty && void save({ body: html })}
              spellCheck={false}
              placeholder={'<!doctype html>\n<html>'}
              className="min-w-0 flex-1 rounded-none border-0 font-mono text-xs leading-relaxed"
            />
          ) : artifact.body.trim() ? (
            // Sandboxed: scripts run, but no same-origin — can't touch the app.
            <iframe title={artifact.title} srcDoc={artifact.body} sandbox="allow-scripts allow-forms allow-popups allow-modals" className="min-w-0 flex-1 border-0 bg-white" />
          ) : (
            <div className="grid min-w-0 flex-1 place-items-center p-8 text-center text-sm text-muted">
              <button type="button" onClick={() => setMode('edit')} className="hover:text-fg">Empty microsite. Switch to Edit to write HTML.</button>
            </div>
          )
        ) : artifact.kind === 'sheet' ? (
          <SheetView key={`${id}-${seed}`} value={artifact.body} editable={mode === 'edit'} onSave={(body) => void save({ body })} />
        ) : artifact.kind === 'file' ? (
          <div className="min-w-0 flex-1 overflow-y-auto p-8">
            {artifact.storageRef ? (
              <div className="mx-auto max-w-2xl">
                {artifact.contentType?.startsWith('image/') ? (
                  <img src={`/api/uploads/${artifact.storageRef}`} alt={artifact.title} className="mb-4 max-h-[60vh] rounded-xl border border-line-subtle" />
                ) : (
                  <div className="mb-4 flex items-center gap-3 rounded-xl border border-line-subtle p-4">
                    <Paperclip size={20} className="shrink-0 text-muted" />
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-sans text-sm text-fg">{artifact.title}</div>
                      <div className="text-xs text-muted">{artifact.contentType ?? 'file'}</div>
                    </div>
                  </div>
                )}
                <div className="flex gap-2">
                  <a href={`/api/uploads/${artifact.storageRef}`} target="_blank" rel="noreferrer" className={buttonClasses({ size: 'sm' })}>
                    Download
                  </a>
                  {isOwner && (
                    <label className={cn(buttonClasses({ size: 'sm', variant: 'outline' }), 'cursor-pointer')}>
                      <input type="file" className="hidden" onChange={(e) => void onPickFile(e.target.files?.[0])} />
                      Replace file
                    </label>
                  )}
                </div>
              </div>
            ) : (
              <label className="mx-auto flex max-w-lg cursor-pointer flex-col items-center gap-2 rounded-2xl border-2 border-dashed border-line-subtle p-12 text-center hover:border-[var(--theme-accent-border)]">
                <input type="file" className="hidden" onChange={(e) => void onPickFile(e.target.files?.[0])} />
                <Paperclip size={22} className="text-muted" />
                <div className="text-sm text-fg">{uploading ? 'Uploading' : 'Click to upload a file'}</div>
                <div className="text-xs text-muted">Up to 25 MB · stored and hosted by Talaria</div>
              </label>
            )}
          </div>
        ) : (
          <div className="grid min-w-0 flex-1 place-items-center p-8 text-center text-sm text-muted">
            {artifact.kind} artifacts are coming soon.
          </div>
        )}
        {showHistory && (
          <div className="w-64 shrink-0 overflow-y-auto border-l border-line-subtle p-3">
            <ArtifactHistory
              id={id}
              onRestore={async (content) => {
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

      <div className="flex items-center gap-2 border-t border-line-subtle px-6 py-2 text-xs text-muted">
        <span>edited {relativeTime(artifact.updatedAt)}{artifact.updatedBy ? ` by ${artifact.updatedBy}` : ''}</span>
        <span className="ml-auto" />
        {mode === 'edit' && artifact.kind !== 'file' && <span className="text-[11px] text-muted">{saving ? 'Saving' : 'Saved'}</span>}
      </div>

      <PermissionsModal
        open={shareOpen}
        onClose={() => setShareOpen(false)}
        kind="artifacts"
        id={id}
        label={artifact.title}
        visibility={artifact.visibility}
        editPolicy={artifact.editPolicy}
        publicSlug={artifact.publicSlug}
        canManage={isOwner}
        onSave={(patch) => save(patch)}
      />
    </div>
  )
}

// Loading stand-in matching the artifact editor layout: toolbar (icon + title
// + buttons) over a centered prose column, so the swap to content doesn't jump.
const PROSE_WIDTHS = ['100%', '92%', '97%', '86%', '95%', '73%', '100%', '90%', '96%', '58%']
function ArtifactPageSkeleton() {
  return (
    <div aria-hidden className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 border-b border-line-subtle px-6 py-4">
        <Skeleton className="h-7 w-7 shrink-0" />
        <Skeleton className="h-5 w-64 max-w-[40%] rounded-full" delay={0.08} />
        <span className="ml-auto flex shrink-0 gap-2">
          <Skeleton className="h-7 w-20" delay={0.16} />
          <Skeleton className="h-7 w-28" delay={0.24} />
        </span>
      </div>
      <div className="mx-auto w-full max-w-[46rem] flex-1 space-y-3.5 px-6 py-8">
        {PROSE_WIDTHS.map((w, i) => (
          <div key={i} style={{ width: w }}>
            <Skeleton className="h-3 w-full rounded-full" delay={i * 0.08} />
          </div>
        ))}
      </div>
    </div>
  )
}

interface Rev { id: string; createdBy: string | null; createdAt: string; size: number }
function ArtifactHistory({ id, onRestore }: { id: string; onRestore: (content: string) => Promise<void> }) {
  const [revs, setRevs] = useState<Rev[]>([])
  const [loading, setLoading] = useState(true)
  useEffect(() => {
    setLoading(true)
    fetch(`/api/history?kind=artifact&id=${id}`)
      .then((r) => (r.ok ? r.json() : { revisions: [] }))
      .then((d) => setRevs((d as { revisions: Rev[] }).revisions))
      .catch(() => setRevs([]))
      .finally(() => setLoading(false))
  }, [id])
  const restore = async (rev: Rev) => {
    const r = await fetch(`/api/history?kind=artifact&id=${id}&rev=${rev.id}`)
    if (!r.ok) return
    const { content } = (await r.json()) as { content: string }
    await onRestore(content)
  }
  return (
    <div>
      <div className="mb-2 text-[11px] uppercase tracking-wide text-muted">History</div>
      {loading ? (
        <SkeletonRows rows={4} className="px-2 py-1" />
      ) : revs.length === 0 ? (
        <div className="text-xs text-muted">No saved revisions yet.</div>
      ) : (
        revs.map((r, i) => (
          <button key={r.id} type="button" onClick={() => void restore(r)} className="block w-full rounded-md px-2 py-1.5 text-left text-xs hover:bg-card" title="Restore this version">
            <div className="text-fg">{i === 0 ? 'Latest' : relativeTime(r.createdAt)}</div>
            <div className="text-[11px] text-muted">{r.createdBy ?? 'unknown'} · {r.size} chars</div>
          </button>
        ))
      )}
    </div>
  )
}

// A spreadsheet-style grid. The sheet body is JSON `string[][]` — row 0 is the
// header row. Edit mode is an editable grid with add/delete row+column; read
// mode renders a table. Autosaves (debounced).
function parseGrid(body: string): string[][] {
  try {
    const g = JSON.parse(body)
    if (Array.isArray(g) && g.length && g.every((r) => Array.isArray(r))) return g.map((r: unknown[]) => r.map((c) => String(c ?? '')))
  } catch {
    /* fall through to a starter grid */
  }
  return [['Column A', 'Column B', 'Column C'], ['', '', ''], ['', '', '']]
}

function SheetView({ value, editable, onSave }: { value: string; editable: boolean; onSave: (body: string) => void }) {
  const [grid, setGrid] = useState<string[][]>(() => parseGrid(value))
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current) }, [])
  const commit = (g: string[][]) => {
    setGrid(g)
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => onSave(JSON.stringify(g)), 600)
  }
  const setCell = (r: number, c: number, v: string) => commit(grid.map((row, ri) => (ri === r ? row.map((cell, ci) => (ci === c ? v : cell)) : row)))
  const addRow = () => commit([...grid, grid[0]!.map(() => '')])
  const addCol = () => commit(grid.map((row, i) => [...row, i === 0 ? `Column ${String.fromCharCode(65 + row.length)}` : '']))
  const delRow = (r: number) => grid.length > 1 && commit(grid.filter((_, i) => i !== r))
  const delCol = (c: number) => grid[0]!.length > 1 && commit(grid.map((row) => row.filter((_, i) => i !== c)))

  const cols = grid[0]?.length ?? 0

  if (!editable) {
    const [head = [], ...rows] = grid
    return (
      <div className="min-w-0 flex-1 overflow-auto p-6">
        <table className="border-collapse text-sm">
          <thead>
            <tr>{head.map((h, i) => <th key={i} className="border border-line-subtle bg-card px-3 py-1.5 text-left font-semibold text-fg">{h}</th>)}</tr>
          </thead>
          <tbody>
            {rows.map((row, ri) => (
              <tr key={ri}>{row.map((cell, ci) => <td key={ci} className="border border-line-subtle px-3 py-1.5 text-fg">{cell}</td>)}</tr>
            ))}
          </tbody>
        </table>
      </div>
    )
  }

  return (
    <div className="min-w-0 flex-1 overflow-auto p-6">
      <table className="border-collapse text-sm">
        <thead>
          <tr>
            {grid[0]!.map((_, c) => (
              <th key={c} className="p-0.5">
                <button type="button" onClick={() => delCol(c)} title="Delete column" className="w-full rounded text-[10px] text-muted hover:text-[color:var(--theme-danger)]">✕</button>
              </th>
            ))}
            <th />
          </tr>
        </thead>
        <tbody>
          {grid.map((row, r) => (
            <tr key={r}>
              {row.map((cell, c) => (
                <td key={c} className="border border-line-subtle p-0">
                  <input
                    value={cell}
                    onChange={(e) => setCell(r, c, e.target.value)}
                    className={cn('w-40 bg-transparent px-2 py-1.5 text-sm text-fg outline-none focus:bg-card', r === 0 && 'font-semibold')}
                    placeholder={r === 0 ? 'Header' : ''}
                  />
                </td>
              ))}
              <td className="pl-1">
                {r > 0 && <button type="button" onClick={() => delRow(r)} title="Delete row" className="text-[10px] text-muted hover:text-[color:var(--theme-danger)]">✕</button>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="mt-3 flex gap-2">
        <Button variant="outline" size="sm" onClick={addRow}><Plus size={12} className="mr-1" /> Row</Button>
        <Button variant="outline" size="sm" onClick={addCol}><Plus size={12} className="mr-1" /> Column</Button>
        <span className="self-center text-[11px] text-muted">{grid.length - 1} rows · {cols} columns</span>
      </div>
    </div>
  )
}
