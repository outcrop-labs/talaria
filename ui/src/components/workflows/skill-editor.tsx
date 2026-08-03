// The Studio's skill editor — one SKILL.md in the full workspace editor
// (rich + Muse drafting + version history). Same contract as the agent-view
// editor: reads live, Hermes picks up saves on the agent's next run.
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { confirm } from '@/components/ui/confirm'
import { QueryError } from '@/components/ui/query-state'
import { getJson } from '@/lib/fetch-json'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'

export function StudioSkillEditor({
  owner,
  ownerLabel,
  name,
  canEdit,
  onClose,
}: {
  owner: string
  ownerLabel: string
  name: string
  canEdit: boolean
  onClose: () => void
}) {
  const qc = useQueryClient()
  // 404 is NOT forgiven into a null here: the editor is opened from a library
  // row, so "no such skill" is a real failure worth naming — and the route
  // sends its reason as `{ error }`, which `readJson` lifts into the message.
  const query = useQuery({
    queryKey: ['skill', owner, name],
    queryFn: (): Promise<{ content: string; files: string[] }> => getJson<{ content: string; files: string[] }>(`/api/skills/${owner}/${name}`),
  })
  const { data } = query
  const [busy, setBusy] = useState(false)

  const save = async (content: string) => {
    setBusy(true)
    try {
      await fetch(`/api/skills/${owner}/${name}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content }),
      })
      await qc.invalidateQueries({ queryKey: ['skill-library'] })
      await qc.invalidateQueries({ queryKey: ['skills'] })
      await qc.invalidateQueries({ queryKey: ['skill', owner, name] })
    } finally {
      setBusy(false)
    }
  }
  const remove = async () => {
    if (!(await confirm({ title: 'Delete skill', message: `Delete "${name}"? Workflows bound to it will flag it as missing.`, confirmLabel: 'Delete', danger: true }))) return
    await fetch(`/api/skills/${owner}/${name}`, { method: 'DELETE', credentials: 'same-origin' })
    await qc.invalidateQueries({ queryKey: ['skill-library'] })
    await qc.invalidateQueries({ queryKey: ['skills'] })
    onClose()
  }

  // The editor seeds ONCE from `value` — don't mount it until content is here.
  // A failed read must never seed it with '': saving from there would replace
  // the real SKILL.md with an empty file.
  if (!data)
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40" onClick={query.isError ? onClose : undefined}>
        <div className="w-full max-w-3xl space-y-3 rounded-2xl bg-[var(--theme-panel)] p-6" onClick={(e) => e.stopPropagation()}>
          {query.isError ? (
            <QueryError
              variant="compact"
              error={query.error}
              title={`Could not open ${name}`}
              onRetry={() => void query.refetch()}
            />
          ) : (
            <>
              <Skeleton className="h-2.5 w-2/3 rounded-full" />
              <Skeleton className="h-2.5 w-full rounded-full" delay={0.12} />
              <Skeleton className="h-2.5 w-3/4 rounded-full" delay={0.24} />
            </>
          )}
        </div>
      </div>
    )

  return (
    <InternalEditorModal
      open
      onClose={onClose}
      title={`${name} · SKILL.md`}
      subtitle={`${ownerLabel} — read live; agents pick up edits on their next run.`}
      value={data.content}
      editable={canEdit}
      saving={busy}
      onSave={save}
      history={{ kind: 'skill', owner, name }}
      muse={{
        kind: 'skill',
        context:
          owner === 'shared'
            ? 'A shared skill available to every agent in the fleet. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.'
            : `A skill for the "${ownerLabel}" agent. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.`,
      }}
      footerExtra={
        canEdit ? (
          <Button variant="ghost" size="sm" onClick={() => void remove()}>
            Delete skill
          </Button>
        ) : undefined
      }
    />
  )
}
