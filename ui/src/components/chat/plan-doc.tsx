import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Maximize2, Minimize2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { GeneratingOverlay } from '@/components/ui/generating'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { saveArtifact, useArtifact } from '@/lib/artifacts'
import { cn } from '@/lib/cn'

// The plan's living document — a real `doc` artifact, side-by-side with the chat.
// One per plan (linked via artifact_links target_type='plan'); found-or-created
// server-side on first open, seeded from the agent's plan template when one is
// bound. Editable on the fly, autosaved, referenceable anywhere in the app.
export function PlanDoc({ planId, syncSignal = 0 }: { planId: string; planTitle?: string | null; syncSignal?: number }) {
  const [docId, setDocId] = useState<string | null>(null)

  useEffect(() => {
    setDocId(null)
    let cancelled = false
    void fetch(`/api/plan/${planId}/doc`, { credentials: 'same-origin' })
      .then((r) => (r.ok ? (r.json() as Promise<{ artifact: { id: string } }>) : null))
      .then((j) => {
        if (!cancelled && j) setDocId(j.artifact.id)
      })
    return () => {
      cancelled = true
    }
  }, [planId])

  return (
    <div className="flex min-w-0 flex-col border-l border-line-subtle">
      {docId ? (
        <DocEditor id={docId} planId={planId} syncSignal={syncSignal} />
      ) : (
        <div className="grid flex-1 place-items-center text-sm text-muted">Preparing the plan document</div>
      )}
    </div>
  )
}

function DocEditor({ id, planId, syncSignal = 0 }: { id: string; planId: string; syncSignal?: number }) {
  const qc = useQueryClient()
  const { data: artifact } = useArtifact(id)
  const editorRef = useRef<RichEditorHandle>(null)
  const [syncing, setSyncing] = useState(false)
  const [syncErr, setSyncErr] = useState<string | null>(null)
  // The editor is uncontrolled after mount — bump to remount on an agent sync.
  const [syncNonce, setSyncNonce] = useState(0)
  // Fullscreen (Esc exits) — same affordance as the artifact/KB editors.
  const [fullscreen, setFullscreen] = useState(false)
  useEffect(() => {
    if (!fullscreen) return
    const h = (e: KeyboardEvent) => e.key === 'Escape' && setFullscreen(false)
    window.addEventListener('keydown', h)
    return () => window.removeEventListener('keydown', h)
  }, [fullscreen])

  const save = async () => {
    const body = editorRef.current?.getMarkdown() ?? artifact?.body ?? ''
    await saveArtifact(id, { body })
    void qc.invalidateQueries({ queryKey: ['artifact', id] })
  }

  // The plan's agent rewrites the document from the conversation so far.
  const sync = async () => {
    setSyncing(true)
    setSyncErr(null)
    try {
      const r = await fetch(`/api/plan/${planId}/doc`, { method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json' }, body: '{}' })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok) throw new Error(j.error ?? `sync failed (${r.status})`)
      await qc.invalidateQueries({ queryKey: ['artifact', id] })
      setSyncNonce((n) => n + 1)
    } catch (e) {
      setSyncErr((e as Error).message)
    } finally {
      setSyncing(false)
    }
  }

  // The document builds as you talk: every landed agent turn triggers a sync.
  // Unsaved manual edits are flushed first so the rewrite starts from them
  // instead of clobbering them. Signal 0 is mount, not a turn.
  const lastSignal = useRef(syncSignal)
  useEffect(() => {
    if (syncSignal === lastSignal.current) return
    lastSignal.current = syncSignal
    void (async () => {
      const md = editorRef.current?.getMarkdown()
      if (md !== undefined && md !== artifact?.body) await save().catch(() => {})
      await sync()
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [syncSignal])

  if (!artifact) return <div className="grid flex-1 place-items-center text-sm text-muted">Loading</div>

  return (
    <div className={cn('flex min-h-0 flex-col', fullscreen ? 'fixed inset-0 z-50 bg-surface' : 'flex-1')}>
      <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-2">
        <span className="text-[11px] uppercase tracking-wide text-muted">Plan document</span>
        <span className="min-w-0 flex-1 truncate text-sm text-fg">{artifact.title}</span>
        <Button size="sm" variant="outline" onClick={() => void sync()} disabled={syncing} title="The agent rewrites the document from the conversation so far">
          {syncing ? 'Syncing' : 'Sync from chat'}
        </Button>
        <span className="shrink-0 text-[11px] text-muted">Auto-saves</span>
        <Link to="/artifacts" className="shrink-0 text-[11px] text-accent hover:underline" title="Open in Artifacts">
          Open ↗
        </Link>
        <Button variant="ghost" size="sm" className="shrink-0" title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'} onClick={() => setFullscreen((v) => !v)}>
          {fullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
        </Button>
      </div>
      {syncErr && <div className="border-b border-line-subtle px-4 py-1.5 text-xs" style={{ color: 'var(--theme-danger)' }}>{syncErr}</div>}
      <div className="relative min-h-0 flex-1 overflow-y-auto">
        {syncing && <GeneratingOverlay label="Rewriting the plan document from the conversation" />}
        <RichEditor
          key={`${id}:${syncNonce}`}
          ref={editorRef}
          value={artifact.body}
          slash
          prose
          autosave
          onSave={() => void save()}
          placeholder="The plan takes shape here: outline goals, scope, and decisions. Draft tickets from it when ready."
          fill
          className="min-w-0 flex-1"
        />
      </div>
    </div>
  )
}
