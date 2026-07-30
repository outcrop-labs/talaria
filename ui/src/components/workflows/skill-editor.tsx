// The Studio's skill editor — one SKILL.md in the full workspace editor
// (rich + Muse drafting + version history). Same contract as the agent-view
// editor: reads live, Hermes picks up saves on the agent's next run.
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { confirm } from '@/components/ui/confirm'
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
  const { data } = useQuery({
    queryKey: ['skill', owner, name],
    queryFn: async () => {
      const r = await fetch(`/api/skills/${owner}/${name}`, { credentials: 'same-origin' })
      if (!r.ok) throw new Error('failed')
      return (await r.json()) as { content: string; files: string[] }
    },
  })
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
  if (!data)
    return (
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
        <div className="w-full max-w-3xl space-y-3 rounded-2xl bg-[var(--theme-panel)] p-6">
          <Skeleton className="h-2.5 w-2/3 rounded-full" />
          <Skeleton className="h-2.5 w-full rounded-full" delay={0.12} />
          <Skeleton className="h-2.5 w-3/4 rounded-full" delay={0.24} />
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
