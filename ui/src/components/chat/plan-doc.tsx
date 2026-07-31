import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Maximize2, Minimize2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { GeneratingOverlay } from '@/components/ui/generating'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { Skeleton } from '@/components/ui/skeleton'
import { saveArtifact, useArtifact } from '@/lib/artifacts'
import { cn } from '@/lib/cn'

/** Doc-page skeleton: toolbar row + prose block, shaped like the editor it
 *  stands in for. The SAME skeleton covers both loading phases (doc lookup,
 *  then the artifact fetch) so the pane never double-swaps mid-load. */
export function PlanDocSkeleton() {
  return (
    <div aria-hidden className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-2">
        <Skeleton className="h-2.5 w-20 rounded-full" />
        <div className="min-w-0 flex-1">
          <Skeleton className="h-3 w-1/2 rounded-full" delay={0.12} />
        </div>
        <Skeleton className="h-7 w-24 shrink-0" delay={0.24} />
        <Skeleton className="h-7 w-14 shrink-0" delay={0.36} />
      </div>
      <div className="min-h-0 flex-1 space-y-3 overflow-hidden p-6">
        <Skeleton className="h-5 w-2/3 rounded-full" />
        {['100%', '94%', '88%', '97%', '82%', '91%', '60%'].map((w, i) => (
          <div key={i} style={{ width: w }}>
            <Skeleton className="h-2.5 w-full rounded-full" delay={0.12 * (i + 1)} />
          </div>
        ))}
      </div>
    </div>
  )
}

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
      {docId ? <DocEditor id={docId} planId={planId} syncSignal={syncSignal} /> : <PlanDocSkeleton />}
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

  if (!artifact) return <PlanDocSkeleton />

  return (
    <div className={cn('flex min-h-0 flex-col', fullscreen ? 'fixed inset-0 z-50 bg-surface' : 'flex-1')}>
      <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-2">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Plan document</span>
        <span className="min-w-0 flex-1 truncate font-sans text-sm text-fg">{artifact.title}</span>
        <Button size="sm" variant="outline" onClick={() => void sync()} disabled={syncing} title="The agent rewrites the document from the conversation so far">
          {syncing ? 'Syncing' : 'Sync from chat'}
        </Button>
        <span className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Auto-saves</span>
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
