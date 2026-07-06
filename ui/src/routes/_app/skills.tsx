import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { cn } from '@/lib/cn'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Panel } from '@/components/ui/panel'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { useSession } from '@/lib/session'

export const Route = createFileRoute('/_app/skills')({
  component: SkillsPage,
})

interface SkillSummary {
  name: string
  description: string
  files: string[]
}

interface OwnerSkills {
  owner: string
  label: string
  source: 'shared' | 'imported' | 'created'
  skills: SkillSummary[]
}

const useSkills = () =>
  useQuery({
    queryKey: ['skills'],
    queryFn: async (): Promise<OwnerSkills[]> => {
      const r = await fetch('/api/skills')
      if (!r.ok) throw new Error('failed to load skills')
      return ((await r.json()) as { owners: OwnerSkills[] }).owners
    },
  })

// Skills are files the agents read live from their mounts — edits apply on the
// next invocation, no restart. Shared skills reach every agent.
function SkillsPage() {
  const { data: session } = useSession()
  const isAdmin = session?.role === 'admin'
  const { data: owners = [], isLoading } = useSkills()
  const [ownerKey, setOwnerKey] = useState<string | null>(null)
  const [skill, setSkill] = useState<string | null>(null)

  const active = owners.find((o) => o.owner === (ownerKey ?? 'shared')) ?? owners[0]

  return (
    <div className="h-full overflow-y-auto p-8">
      <div className="mx-auto max-w-5xl space-y-8">
        <h1 className="mercury-text text-2xl font-semibold">Skills</h1>

        {isLoading ? (
          <div className="text-sm text-muted">Loading skills…</div>
        ) : owners.length === 0 ? (
          <EmptyState icon="✦" title="No agents yet" hint="Import your stack on the Agents page first." />
        ) : (
          <div className="flex items-start gap-6">
            <div className="w-52 shrink-0 space-y-1">
              {owners.map((o) => (
                <button
                  key={o.owner}
                  type="button"
                  onClick={() => {
                    setOwnerKey(o.owner)
                    setSkill(null)
                  }}
                  className={cn(
                    'flex w-full items-baseline gap-2 rounded-lg px-3 py-2 text-left text-sm transition-colors',
                    active?.owner === o.owner ? 'bg-card text-fg' : 'text-muted hover:text-fg',
                  )}
                >
                  <span className="truncate">{o.label}</span>
                  <span className="ml-auto text-xs">{o.skills.length}</span>
                </button>
              ))}
            </div>

            <div className="min-w-0 flex-1 space-y-6">
              {active && (
                <OwnerPanel
                  key={active.owner}
                  owner={active}
                  isAdmin={isAdmin}
                  selected={skill}
                  onSelect={setSkill}
                />
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

function OwnerPanel({
  owner,
  isAdmin,
  selected,
  onSelect,
}: {
  owner: OwnerSkills
  isAdmin: boolean
  selected: string | null
  onSelect: (s: string | null) => void
}) {
  const qc = useQueryClient()
  const [newName, setNewName] = useState('')
  const [err, setErr] = useState<string | null>(null)

  const create = async () => {
    const name = newName.trim().toLowerCase().replace(/\s+/g, '-')
    if (!name) return
    setErr(null)
    const r = await fetch(`/api/skills/${owner.owner}/${name}`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content: `# ${name}\n\ndescription: what this skill is for\n\n## Steps\n\n1. …\n` }),
    }).catch(() => null)
    if (!r?.ok) return setErr('could not create the skill')
    setNewName('')
    await qc.invalidateQueries({ queryKey: ['skills'] })
    onSelect(name)
  }

  return (
    <>
      <Panel>
        <div className="mb-4 flex items-center gap-3">
          <span className="text-sm font-semibold text-fg">{owner.label}</span>
          <span className="text-xs text-muted">
            {owner.source === 'shared' ? 'read by every agent' : owner.source === 'imported' ? 'department skills' : 'fleet-managed skills'}
            {' · live — agents pick up edits on the next run'}
          </span>
          {isAdmin && (
            <span className="ml-auto flex items-center gap-2">
              <Input
                size="sm"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                placeholder="new-skill-name"
                className="w-44"
                onKeyDown={(e) => e.key === 'Enter' && void create()}
              />
              <Button size="sm" onClick={() => void create()} disabled={!newName.trim()}>
                Add
              </Button>
            </span>
          )}
        </div>
        {owner.skills.length === 0 ? (
          <EmptyState icon="✦" title="No skills yet" hint={isAdmin ? 'Add one above — it lands as SKILL.md in the mount.' : undefined} />
        ) : (
          <div className="divide-y divide-line-subtle">
            {owner.skills.map((s) => (
              <button
                key={s.name}
                type="button"
                onClick={() => onSelect(selected === s.name ? null : s.name)}
                className="flex w-full items-baseline gap-3 py-3 text-left"
              >
                <span className={cn('shrink-0 text-sm font-medium', selected === s.name ? 'text-accent' : 'text-fg')}>
                  {s.name}
                </span>
                <span className="min-w-0 truncate text-sm text-muted">{s.description}</span>
                {s.files.length > 1 && <span className="ml-auto shrink-0 text-xs text-muted">{s.files.length} files</span>}
              </button>
            ))}
          </div>
        )}
        {err && (
          <div className="mt-2 text-xs" style={{ color: 'var(--theme-danger)' }}>
            {err}
          </div>
        )}
      </Panel>

      {selected && <SkillEditor owner={owner.owner} name={selected} isAdmin={isAdmin} onClose={() => onSelect(null)} />}
    </>
  )
}

function SkillEditor({ owner, name, isAdmin, onClose }: { owner: string; name: string; isAdmin: boolean; onClose: () => void }) {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['skill', owner, name],
    queryFn: async (): Promise<{ content: string; files: string[] }> => {
      const r = await fetch(`/api/skills/${owner}/${name}`)
      if (!r.ok) throw new Error('failed to load')
      return r.json()
    },
  })
  const [busy, setBusy] = useState(false)

  const save = async (content: string) => {
    setBusy(true)
    try {
      await fetch(`/api/skills/${owner}/${name}`, {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content }),
      })
      await qc.invalidateQueries({ queryKey: ['skills'] })
      await qc.invalidateQueries({ queryKey: ['skill', owner, name] })
    } finally {
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!confirm(`Delete the "${name}" skill? The whole directory goes away.`)) return
    await fetch(`/api/skills/${owner}/${name}`, { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['skills'] })
    onClose()
  }

  const extras = (data?.files ?? []).filter((f) => f !== 'SKILL.md')
  return (
    <InternalEditorModal
      open
      onClose={onClose}
      title={`${name} · SKILL.md`}
      subtitle={
        (extras.length ? `Bundled files: ${extras.join(', ')}. ` : '') +
        'Skills are read live — agents pick up edits on their next run.'
      }
      value={data?.content ?? ''}
      editable={isAdmin}
      saving={busy}
      onSave={save}
      history={{ kind: 'skill', owner, name }}
      footerExtra={isAdmin ? <Button variant="ghost" size="sm" onClick={() => void remove()}>Delete skill</Button> : undefined}
    />
  )
}
