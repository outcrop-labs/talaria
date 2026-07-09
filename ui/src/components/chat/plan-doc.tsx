import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { saveArtifact, useArtifact } from '@/lib/artifacts'

// The plan's living document — a real `doc` artifact, side-by-side with the chat.
// One per plan (linked via artifact_links target_type='plan'); found-or-created
// server-side on first open, seeded from the agent's plan template when one is
// bound. Editable on the fly, autosaved, referenceable anywhere in the app.
export function PlanDoc({ planId }: { planId: string; planTitle?: string | null }) {
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
        <DocEditor id={docId} planId={planId} />
      ) : (
        <div className="grid flex-1 place-items-center text-sm text-muted">Preparing the plan document…</div>
      )}
    </div>
  )
}

function DocEditor({ id, planId }: { id: string; planId: string }) {
  const qc = useQueryClient()
  const { data: artifact } = useArtifact(id)
  const editorRef = useRef<RichEditorHandle>(null)
  const [syncing, setSyncing] = useState(false)
  const [syncErr, setSyncErr] = useState<string | null>(null)
  // The editor is uncontrolled after mount — bump to remount on an agent sync.
  const [syncNonce, setSyncNonce] = useState(0)

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

  if (!artifact) return <div className="grid flex-1 place-items-center text-sm text-muted">Loading…</div>

  return (
    <>
      <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-2">
        <span className="text-[11px] uppercase tracking-wide text-muted">Plan document</span>
        <span className="min-w-0 flex-1 truncate text-sm text-fg">{artifact.title}</span>
        <Button size="sm" variant="outline" onClick={() => void sync()} disabled={syncing} title="The agent rewrites the document from the conversation so far">
          {syncing ? 'Syncing…' : 'Sync from chat'}
        </Button>
        <span className="shrink-0 text-[11px] text-muted">Auto-saves</span>
        <Link to="/artifacts" className="shrink-0 text-[11px] text-accent hover:underline" title="Open in Artifacts">
          Open ↗
        </Link>
      </div>
      {syncErr && <div className="border-b border-line-subtle px-4 py-1.5 text-xs" style={{ color: 'var(--theme-danger)' }}>{syncErr}</div>}
      <div className="min-h-0 flex-1 overflow-y-auto">
        <RichEditor
          key={`${id}:${syncNonce}`}
          ref={editorRef}
          value={artifact.body}
          slash
          prose
          autosave
          onSave={() => void save()}
          placeholder="The plan takes shape here — outline goals, scope, and decisions. Draft tickets from it when ready."
          fill
          className="min-w-0 flex-1"
        />
      </div>
    </>
  )
}
