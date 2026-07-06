import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Plus, FileText, Bot, Globe, Lock, Users, Star, Trash2, History } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { EmptyState } from '@/components/ui/empty-state'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { InlineCreate } from '@/components/ui/inline-create'
import { cn } from '@/lib/cn'
import { relativeTime } from '@/lib/fleet'
import {
  createDoc,
  createSpace,
  deleteDoc,
  saveDoc,
  useDoc,
  useDocs,
  useSpaces,
  type KbDocMeta,
} from '@/lib/kb'

export const Route = createFileRoute('/_app/knowledge')({
  component: KnowledgePage,
})

// The knowledgebase — an Outline-style markdown drive. Spaces on the left, a
// WYSIWYG doc in the middle. Official docs feed the org brain (agents ground on
// them); agent-kind docs start from an OKF scaffold.
function KnowledgePage() {
  const qc = useQueryClient()
  const { data: spaces = [] } = useSpaces()
  const [spaceId, setSpaceId] = useState<string | null>(null)
  const [docId, setDocId] = useState<string | null>(null)
  const activeSpace = spaces.find((s) => s.id === spaceId) ?? spaces[0]
  const { data: docs = [] } = useDocs(activeSpace?.id ?? null)

  useEffect(() => {
    if (!spaceId && spaces[0]) setSpaceId(spaces[0].id)
  }, [spaces, spaceId])

  const newSpace = async (name: string) => {
    const { space } = await createSpace(name)
    await qc.invalidateQueries({ queryKey: ['kb-spaces'] })
    if (space) setSpaceId(space.id)
  }
  const newDoc = async (kind: 'human' | 'agent') => {
    if (!activeSpace) return
    const { doc } = await createDoc(activeSpace.id, { kind, title: 'Untitled' })
    await qc.invalidateQueries({ queryKey: ['kb-docs', activeSpace.id] })
    if (doc) setDocId(doc.id)
  }

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex h-full w-64 shrink-0 flex-col border-r border-line-subtle bg-sidebar">
        <div className="border-b border-line-subtle p-3">
          <InlineCreate label="New space" placeholder="space name" onSubmit={(v) => void newSpace(v)} className="w-full" />
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">
          {spaces.length === 0 ? (
            <div className="px-2 py-6 text-center text-xs text-muted">No spaces yet.</div>
          ) : (
            spaces.map((s) => (
              <div key={s.id} className="mb-2">
                <button
                  type="button"
                  onClick={() => setSpaceId(s.id)}
                  className={cn('flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm', activeSpace?.id === s.id ? 'text-fg' : 'text-muted hover:text-fg')}
                >
                  <span>{s.icon ?? '📚'}</span>
                  <span className="truncate font-medium">{s.name}</span>
                </button>
                {activeSpace?.id === s.id && (
                  <DocTree docs={docs} activeId={docId} onSelect={setDocId} onNew={newDoc} />
                )}
              </div>
            ))
          )}
        </div>
      </aside>

      <main className="min-h-0 min-w-0 flex-1">
        {docId ? (
          <DocEditor key={docId} docId={docId} onDeleted={() => setDocId(null)} />
        ) : (
          <EmptyState
            icon="❖"
            title={activeSpace ? `${activeSpace.name}` : 'Knowledge'}
            hint={activeSpace ? 'Pick a doc, or create one below the space.' : 'Create a space to start writing.'}
          />
        )}
      </main>
    </div>
  )
}

function DocTree({ docs, activeId, onSelect, onNew }: { docs: KbDocMeta[]; activeId: string | null; onSelect: (id: string) => void; onNew: (k: 'human' | 'agent') => void }) {
  const roots = docs.filter((d) => !d.parentId)
  return (
    <div className="ml-2 mt-0.5 space-y-0.5 border-l border-line-subtle pl-2">
      {roots.map((d) => (
        <button
          key={d.id}
          type="button"
          onClick={() => onSelect(d.id)}
          className={cn('flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs', activeId === d.id ? 'bg-card text-fg' : 'text-muted hover:text-fg')}
        >
          {d.kind === 'agent' ? <Bot size={12} /> : <FileText size={12} />}
          <span className="min-w-0 flex-1 truncate">{d.title}</span>
          {d.official && <Star size={11} className="shrink-0 text-[color:var(--theme-warning)]" />}
        </button>
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

function DocEditor({ docId, onDeleted }: { docId: string; onDeleted: () => void }) {
  const qc = useQueryClient()
  const { data: doc } = useDoc(docId)
  const editorRef = useRef<RichEditorHandle>(null)
  const [title, setTitle] = useState('')
  const [dirty, setDirty] = useState(false)
  const [saving, setSaving] = useState(false)
  const [showHistory, setShowHistory] = useState(false)
  useEffect(() => {
    if (doc) setTitle(doc.title)
  }, [doc])

  const save = async (patch: Parameters<typeof saveDoc>[1]) => {
    setSaving(true)
    try {
      await saveDoc(docId, patch)
      await qc.invalidateQueries({ queryKey: ['kb-doc', docId] })
      await qc.invalidateQueries({ queryKey: ['kb-docs', doc?.spaceId] })
      setDirty(false)
    } finally {
      setSaving(false)
    }
  }
  const saveBody = () => save({ title, body: editorRef.current?.getMarkdown() ?? doc?.body ?? '' })

  if (!doc) return <div className="p-8 text-sm text-muted">Loading…</div>

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-line-subtle px-6 py-3">
        <Input
          value={title}
          onChange={(e) => {
            setTitle(e.target.value)
            setDirty(true)
          }}
          onBlur={() => dirty && void saveBody()}
          className="min-w-0 flex-1 border-0 bg-transparent text-lg font-semibold focus:border-0"
          placeholder="Untitled"
        />
        {doc.kind === 'agent' && <span className="shrink-0 rounded border border-line-subtle px-1.5 text-[10px] uppercase tracking-wide text-muted">OKF</span>}
        <Select value={doc.visibility} size="sm" onChange={(e) => void save({ visibility: e.target.value as 'private' | 'org' | 'public' })} className="shrink-0">
          <option value="private">Private</option>
          <option value="org">Org</option>
          <option value="public">Public</option>
        </Select>
        <Button
          variant={doc.official ? 'primary' : 'outline'}
          size="sm"
          className="shrink-0"
          onClick={() => void save({ official: !doc.official })}
          title="Official docs are indexed into the organization brain agents ground on"
        >
          <Star size={13} className="mr-1" /> {doc.official ? 'Official' : 'Make official'}
        </Button>
        <Button variant="ghost" size="sm" className="shrink-0" onClick={() => setShowHistory((v) => !v)}>
          <History size={14} />
        </Button>
        <Button variant="ghost" size="sm" className="shrink-0" onClick={() => confirm(`Delete "${doc.title}"?`) && (void deleteDoc(docId).then(onDeleted))}>
          <Trash2 size={14} />
        </Button>
      </div>

      {doc.visibility === 'public' && doc.publicSlug && (
        <div className="flex items-center gap-2 border-b border-line-subtle bg-card/40 px-6 py-1.5 text-xs text-muted">
          <Globe size={12} /> Public link:
          <code className="text-fg">{typeof window !== 'undefined' ? `${window.location.origin}/kb/${doc.publicSlug}` : `/kb/${doc.publicSlug}`}</code>
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <div className="min-w-0 flex-1 overflow-y-auto p-6">
          <RichEditor
            key={docId}
            ref={editorRef}
            value={doc.body}
            onSave={() => void saveBody()}
            placeholder={doc.kind === 'agent' ? 'OKF-structured knowledge for agents…' : 'Write…'}
            minHeight="60vh"
          />
        </div>
        {showHistory && (
          <div className="w-64 shrink-0 overflow-y-auto border-l border-line-subtle p-3">
            <HistoryRail docId={docId} />
          </div>
        )}
      </div>

      <div className="flex items-center gap-2 border-t border-line-subtle px-6 py-2 text-xs text-muted">
        <VisibilityIcon v={doc.visibility} />
        <span>edited {relativeTime(doc.updatedAt)}{doc.updatedBy ? ` by ${doc.updatedBy}` : ''}</span>
        <span className="ml-auto" />
        <Button size="sm" onClick={() => void saveBody()} disabled={saving}>
          {saving ? 'Saving…' : 'Save'}
        </Button>
      </div>
    </div>
  )
}

const VisibilityIcon = ({ v }: { v: 'private' | 'org' | 'public' }) =>
  v === 'public' ? <Globe size={12} /> : v === 'private' ? <Lock size={12} /> : <Users size={12} />

interface Rev {
  id: string
  createdBy: string | null
  createdAt: string
  size: number
}
function HistoryRail({ docId }: { docId: string }) {
  const [revs, setRevs] = useState<Rev[]>([])
  useEffect(() => {
    // kb-doc history keys on the doc id (like memory).
    fetch(`/api/history?kind=kb-doc&id=${docId}`)
      .then((r) => (r.ok ? r.json() : { revisions: [] }))
      .then((d) => setRevs((d as { revisions: Rev[] }).revisions))
      .catch(() => setRevs([]))
  }, [docId])
  return (
    <div>
      <div className="mb-2 text-[11px] uppercase tracking-wide text-muted">History</div>
      {revs.length === 0 ? (
        <div className="text-xs text-muted">No saved revisions yet.</div>
      ) : (
        revs.map((r, i) => (
          <div key={r.id} className="py-1.5 text-xs">
            <div className="text-fg">{i === 0 ? 'Latest' : relativeTime(r.createdAt)}</div>
            <div className="text-[11px] text-muted">{r.createdBy ?? 'unknown'} · {r.size} chars</div>
          </div>
        ))
      )}
    </div>
  )
}
