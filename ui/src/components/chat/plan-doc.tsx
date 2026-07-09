import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { RichEditor, type RichEditorHandle } from '@/components/ui/rich-editor'
import { attachArtifact, createArtifact, saveArtifact, useArtifact, useTargetArtifacts } from '@/lib/artifacts'

// The plan's living document — a real `doc` artifact, side-by-side with the chat.
// One per plan (linked via artifact_links target_type='plan'); created on first
// open. Editable on the fly, autosaved, and referenceable anywhere in the app.
export function PlanDoc({ planId, planTitle }: { planId: string; planTitle: string | null }) {
  const qc = useQueryClient()
  const { data: linked, isLoading } = useTargetArtifacts('plan', planId)
  const [docId, setDocId] = useState<string | null>(null)
  const creating = useRef(false)

  // Adopt the existing linked doc, or create one the first time a plan is opened.
  useEffect(() => {
    if (isLoading) return
    const existing = linked?.find((a) => a.kind === 'doc')
    if (existing) {
      setDocId(existing.id)
      return
    }
    if (creating.current || docId) return
    creating.current = true
    void (async () => {
      const { artifact } = await createArtifact({ kind: 'doc', title: `Plan — ${planTitle || 'Untitled'}` })
      await attachArtifact(artifact.id, 'plan', planId)
      await qc.invalidateQueries({ queryKey: ['artifacts-for', 'plan', planId] })
      setDocId(artifact.id)
      creating.current = false
    })()
  }, [isLoading, linked, planId, planTitle, docId, qc])

  return (
    <div className="flex min-w-0 flex-col border-l border-line-subtle">
      {docId ? (
        <DocEditor id={docId} />
      ) : (
        <div className="grid flex-1 place-items-center text-sm text-muted">Preparing the plan document…</div>
      )}
    </div>
  )
}

function DocEditor({ id }: { id: string }) {
  const qc = useQueryClient()
  const { data: artifact } = useArtifact(id)
  const editorRef = useRef<RichEditorHandle>(null)

  const save = async () => {
    const body = editorRef.current?.getMarkdown() ?? artifact?.body ?? ''
    await saveArtifact(id, { body })
    void qc.invalidateQueries({ queryKey: ['artifact', id] })
  }

  if (!artifact) return <div className="grid flex-1 place-items-center text-sm text-muted">Loading…</div>

  return (
    <>
      <div className="flex items-center gap-2 border-b border-line-subtle px-4 py-2">
        <span className="text-[11px] uppercase tracking-wide text-muted">Plan document</span>
        <span className="min-w-0 flex-1 truncate text-sm text-fg">{artifact.title}</span>
        <span className="shrink-0 text-[11px] text-muted">Auto-saves</span>
        <Link to="/artifacts" className="shrink-0 text-[11px] text-accent hover:underline" title="Open in Artifacts">
          Open ↗
        </Link>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <RichEditor
          key={id}
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
