import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, Trash2 } from 'lucide-react'
import { confirm } from '@/components/ui/confirm'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { EmptyState } from '@/components/ui/empty-state'
import { InfoTip } from '@/components/ui/info-tip'
import { Markdown } from '@/components/ui/markdown'
import { CronsPanel } from '@/components/fleet/agent-crons'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { cn } from '@/lib/cn'
import { type Assistant } from '@/lib/assistant'

// The assistant's working parts — Schedules, Skills, Memory — rendered inline
// on the Settings › Assistant tab (identity/model/power live in
// assistant-section above). All server calls are owner-scoped; nothing here
// needs the admin role.
const TABS = ['Schedules', 'Skills', 'Memory'] as const
type Tab = (typeof TABS)[number]

export function AssistantPanels({ assistant }: { assistant: Assistant }) {
  const [tab, setTab] = useState<Tab>('Schedules')
  return (
    <div className="space-y-5">
      <div className="flex gap-1 border-b border-line-subtle pb-2">
        {TABS.map((t) => (
          <button
            key={t}
            type="button"
            onClick={() => setTab(t)}
            className={cn(
              'rounded-lg px-3 py-1 text-sm transition-colors',
              tab === t ? 'bg-card font-medium text-fg' : 'text-muted hover:text-fg',
            )}
          >
            {t}
          </button>
        ))}
      </div>
      {tab === 'Schedules' && <CronsPanel agentId={assistant.id} />}
      {tab === 'Skills' && <SkillsTab assistant={assistant} />}
      {tab === 'Memory' && <MemoryTab assistant={assistant} />}
    </div>
  )
}

interface SkillSummary {
  name: string
  description: string
}

const SKILL_TEMPLATE = (name: string) => `# ${name}

Describe when your assistant should use this skill and how.

## Steps
1. 
`

function SkillsTab({ assistant }: { assistant: Assistant }) {
  const qc = useQueryClient()
  const { data: skills = [], isLoading } = useQuery({
    queryKey: ['assistant-skills', assistant.slug],
    queryFn: async (): Promise<SkillSummary[]> => {
      const r = await fetch('/api/skills', { credentials: 'same-origin' })
      if (!r.ok) return []
      const owners = ((await r.json()) as { owners: Array<{ owner: string; skills: SkillSummary[] }> }).owners
      return owners.find((o) => o.owner === assistant.slug)?.skills ?? []
    },
  })
  const [open, setOpen] = useState<string | null>(null)
  const [newName, setNewName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = async () => qc.invalidateQueries({ queryKey: ['assistant-skills', assistant.slug] })

  const save = async (name: string, body: string) => {
    setBusy(true)
    setError(null)
    try {
      const r = await fetch(`/api/skills/${assistant.slug}/${name}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: body }),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) return setError(j.error ?? 'could not save')
      await refresh()
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-1.5">
        <span className="text-xs uppercase tracking-wide text-muted">Skills</span>
        <InfoTip text="Step-by-step playbooks your assistant follows for recurring jobs: weekly summaries, travel planning, whatever you teach it." />
      </div>
      {isLoading ? null : skills.length === 0 ? (
        <EmptyState icon="✦" title="No skills yet" hint="Teach it its first playbook below." />
      ) : (
        <ul className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
          {skills.map((s) => (
            <li key={s.name}>
              <button
                type="button"
                onClick={() => setOpen(s.name)}
                className="flex w-full items-baseline gap-2 px-3 py-2 text-left transition-colors hover:bg-card"
              >
                <span className="text-sm text-fg">{s.name}</span>
                <span className="min-w-0 flex-1 truncate text-xs text-muted">{s.description}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex items-center gap-2">
        <Input
          size="sm"
          value={newName}
          onChange={(e) => setNewName(e.target.value.toLowerCase().replace(/[^a-z0-9._-]/g, '-'))}
          placeholder="new-skill-name"
        />
        <Button
          size="sm"
          variant="outline"
          disabled={busy || !/^[a-z0-9][a-z0-9._-]*$/.test(newName)}
          onClick={() =>
            void save(newName, SKILL_TEMPLATE(newName)).then(() => {
              setOpen(newName)
              setNewName('')
            })
          }
        >
          <Plus size={14} /> Add
        </Button>
      </div>
      {error && <p className="text-xs text-[color:var(--theme-danger)]">{error}</p>}
      {open !== null && <SkillEditor assistant={assistant} name={open} onClose={() => setOpen(null)} onChanged={() => void refresh()} />}
    </div>
  )
}

// The full workspace editor (history + diff + restore) for one owned skill.
function SkillEditor({
  assistant,
  name,
  onClose,
  onChanged,
}: {
  assistant: Assistant
  name: string
  onClose: () => void
  onChanged: () => void
}) {
  const [busy, setBusy] = useState(false)
  const { data } = useQuery({
    queryKey: ['assistant-skill', assistant.slug, name],
    queryFn: async (): Promise<{ content: string }> => {
      const r = await fetch(`/api/skills/${assistant.slug}/${name}`, { credentials: 'same-origin' })
      return ((await r.json().catch(() => null)) as { content: string } | null) ?? { content: '' }
    },
  })

  const save = async (md: string) => {
    setBusy(true)
    try {
      await fetch(`/api/skills/${assistant.slug}/${name}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: md }),
      })
      onChanged()
    } finally {
      setBusy(false)
    }
  }

  const remove = async () => {
    if (!(await confirm({ title: 'Remove skill', message: `Remove the "${name}" skill?`, confirmLabel: 'Remove', danger: true }))) return
    await fetch(`/api/skills/${assistant.slug}/${name}`, { method: 'DELETE', credentials: 'same-origin' })
    onChanged()
    onClose()
  }

  if (data === undefined) return null
  return (
    <InternalEditorModal
      open
      onClose={onClose}
      title={`${name} · SKILL.md`}
      subtitle="A playbook your assistant follows. Edits are live on its next run."
      value={data.content}
      editable
      saving={busy}
      onSave={save}
      history={{ kind: 'skill', owner: assistant.slug, name }}
      muse={{ kind: 'skill', context: `A skill for ${assistant.displayName}, a personal AI assistant.` }}
      footerExtra={
        <Button variant="ghost" size="sm" onClick={() => void remove()}>
          <Trash2 size={14} className="mr-1.5" /> Delete skill
        </Button>
      }
    />
  )
}

function MemoryTab({ assistant }: { assistant: Assistant }) {
  const qc = useQueryClient()
  const { data, isLoading } = useQuery({
    queryKey: ['assistant-memory', assistant.id],
    queryFn: async (): Promise<{ content: string; error?: string }> => {
      const r = await fetch(`/api/memory/${assistant.id}`, { credentials: 'same-origin' })
      return ((await r.json().catch(() => null)) as { content: string; error?: string } | null) ?? { content: '', error: 'could not load memory' }
    },
  })
  const [editing, setEditing] = useState(false)
  const [busy, setBusy] = useState(false)

  const save = async (md: string) => {
    setBusy(true)
    try {
      await fetch(`/api/memory/${assistant.id}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: md }),
      })
      await qc.invalidateQueries({ queryKey: ['assistant-memory', assistant.id] })
    } finally {
      setBusy(false)
    }
  }

  if (isLoading) return <div className="text-sm text-muted">Loading</div>
  if (data?.error)
    return <EmptyState icon="◌" title="Memory unavailable" hint={assistant.running ? data.error : 'Start your assistant to read its memory.'} />

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-1.5">
        <span className="text-xs uppercase tracking-wide text-muted">Memory</span>
        <InfoTip text="What it remembers about you and your work. It updates this itself as you go, and you can edit or prune it any time. Every save is snapshotted, so nothing is ever lost." />
      </div>
      {data?.content ? (
        <div className="max-h-72 overflow-y-auto rounded-lg border border-line-subtle p-3 text-sm">
          <Markdown>{data.content}</Markdown>
        </div>
      ) : (
        <EmptyState icon="◌" title="Nothing remembered yet" hint="It writes things down as you work together." />
      )}
      <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
        Open editor
      </Button>
      {editing && (
        <InternalEditorModal
          open
          onClose={() => setEditing(false)}
          title={`${assistant.displayName} · Memory`}
          subtitle="Your assistant maintains this itself; your edits are snapshotted and revertible."
          value={data?.content ?? ''}
          editable
          saving={busy}
          onSave={save}
          history={{ kind: 'memory', id: assistant.id }}
          muse={{ kind: 'memory', context: `The memory of ${assistant.displayName}, a personal AI assistant.` }}
        />
      )}
    </div>
  )
}
