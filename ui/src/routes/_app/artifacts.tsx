import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { FileText, Table, Globe2, Paperclip, Trash2, History, Maximize2, Minimize2, MoreHorizontal, Plus, Star, type LucideIcon } from 'lucide-react'
import { Button, buttonClasses } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { EmptyState } from '@/components/ui/empty-state'
import { EmojiPicker } from '@/components/ui/emoji-picker'
import { Markdown } from '@/components/ui/markdown'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { PermissionsModal } from '@/components/kb/permissions-modal'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import { useSession } from '@/lib/session'
import { createArtifact, deleteArtifact, saveArtifact, uploadFile, useArtifact, useArtifacts, type ArtifactKind } from '@/lib/artifacts'

export const Route = createFileRoute('/_app/artifacts')({
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

function ArtifactsPage() {
  const qc = useQueryClient()
  const { data: artifacts = [] } = useArtifacts()
  const [activeId, setActiveId] = useState<string | null>(null)
  const [newOpen, setNewOpen] = useState(false)

  const create = async (kind: ArtifactKind) => {
    setNewOpen(false)
    const { artifact } = await createArtifact({ kind, title: 'Untitled' })
    await qc.invalidateQueries({ queryKey: ['artifacts'] })
    if (artifact) setActiveId(artifact.id)
  }

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex h-full w-72 shrink-0 flex-col border-r border-line-subtle bg-sidebar">
        <div className="relative flex items-center justify-between border-b border-line-subtle p-3">
          <span className="text-sm font-semibold text-fg">Artifacts</span>
          <Button size="sm" onClick={() => setNewOpen((v) => !v)}>
            <Plus size={13} className="mr-1" /> New
          </Button>
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
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {artifacts.length === 0 ? (
            <div className="px-2 py-6 text-center text-xs text-muted">No artifacts yet.</div>
          ) : (
            artifacts.map((a) => {
              const Icon = KIND_ICON[a.kind]
              return (
                <button
                  key={a.id}
                  type="button"
                  onClick={() => setActiveId(a.id)}
                  className={cn('flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm', activeId === a.id ? 'bg-card text-fg' : 'text-muted hover:text-fg')}
                >
                  {a.icon ? <span className="text-[15px] leading-none">{a.icon}</span> : <Icon size={14} className="shrink-0" />}
                  <span className="min-w-0 flex-1 truncate">{a.title}</span>
                  <span className="shrink-0 text-[10px] uppercase tracking-wide text-muted">{a.kind}</span>
                </button>
              )
            })
          )}
        </div>
      </aside>
      <main className="min-h-0 min-w-0 flex-1">
        {activeId ? (
          <ArtifactEditor key={activeId} id={activeId} onDeleted={() => setActiveId(null)} />
        ) : (
          <EmptyState icon="◆" title="Artifacts" hint="Create an artifact, or pick one from the list." />
        )}
      </main>
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

  if (!artifact) return <div className="p-8 text-sm text-muted">Loading…</div>

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
            className="min-w-0 flex-1 border-0 bg-transparent text-lg font-semibold focus:border-0"
            placeholder="Untitled"
          />
        ) : (
          <h1 className="min-w-0 flex-1 truncate text-lg font-semibold text-fg">{artifact.title}</h1>
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
              <button
                type="button"
                onClick={async () => {
                  setMenuOpen(false)
                  if (!confirm(`Delete "${artifact.title}"?`)) return
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
            <RichEditor key={`${id}-${seed}`} ref={editorRef} value={artifact.body} slash prose autosave onSave={() => void saveBody()} placeholder="Draft your artifact…" fill className="min-w-0 flex-1" />
          ) : (
            <div className="re-prose min-w-0 flex-1 overflow-y-auto">
              {artifact.body.trim() ? (
                <Markdown className="tiptap">{artifact.body}</Markdown>
              ) : (
                <div className="mx-auto max-w-[46rem] px-6 py-8">
                  <button type="button" onClick={() => setMode('edit')} className="text-sm text-muted hover:text-fg">
                    Empty artifact — click to start.
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
              placeholder={'<!doctype html>\n<html>…'}
              className="min-w-0 flex-1 rounded-none border-0 font-mono text-xs leading-relaxed"
            />
          ) : artifact.body.trim() ? (
            // Sandboxed: scripts run, but no same-origin — can't touch the app.
            <iframe title={artifact.title} srcDoc={artifact.body} sandbox="allow-scripts allow-forms allow-popups allow-modals" className="min-w-0 flex-1 border-0 bg-white" />
          ) : (
            <div className="grid min-w-0 flex-1 place-items-center p-8 text-center text-sm text-muted">
              <button type="button" onClick={() => setMode('edit')} className="hover:text-fg">Empty microsite — switch to Edit to write HTML.</button>
            </div>
          )
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
                      <div className="truncate text-sm text-fg">{artifact.title}</div>
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
                <div className="text-sm text-fg">{uploading ? 'Uploading…' : 'Click to upload a file'}</div>
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
        {mode === 'edit' && (artifact.kind === 'doc' || artifact.kind === 'microsite') && <span className="text-[11px] text-muted">{saving ? 'Saving…' : 'Saved'}</span>}
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

interface Rev { id: string; createdBy: string | null; createdAt: string; size: number }
function ArtifactHistory({ id, onRestore }: { id: string; onRestore: (content: string) => Promise<void> }) {
  const [revs, setRevs] = useState<Rev[]>([])
  useEffect(() => {
    fetch(`/api/history?kind=artifact&id=${id}`)
      .then((r) => (r.ok ? r.json() : { revisions: [] }))
      .then((d) => setRevs((d as { revisions: Rev[] }).revisions))
      .catch(() => setRevs([]))
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
      {revs.length === 0 ? (
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
