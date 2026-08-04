import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, Trash2 } from 'lucide-react'
import { confirm } from '@/components/ui/confirm'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { EmptyState } from '@/components/ui/empty-state'
import { Markdown } from '@/components/ui/markdown'
import { Modal } from '@/components/ui/modal'
import { QueryError, QueryState } from '@/components/ui/query-state'
import { SectionHeader } from '@/components/ui/section-header'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { Tabs } from '@/components/ui/tabs'
import { CronsPanel } from '@/components/fleet/agent-crons'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { getJson, getList } from '@/lib/fetch-json'
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
      <Tabs items={TABS.map((t) => ({ id: t, label: t }))} value={tab} onChange={setTab} className="border-b border-line pb-2" />
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
  // `owners` with no entry for this assistant IS a real empty; a 5xx is not.
  // Narrowing happens after the read so the failure can still reject.
  const query = useQuery({
    queryKey: ['assistant-skills', assistant.slug],
    queryFn: async (): Promise<SkillSummary[]> => {
      const owners = await getList<{ owner: string; skills: SkillSummary[] }>('/api/skills', 'owners')
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
      <SectionHeader
        className="mb-0"
        title="Skills"
        info="Step-by-step playbooks your assistant follows for recurring jobs: weekly summaries, travel planning, whatever you teach it."
        // The count is meta ABOUT the read, so it only exists once the read
        // resolved: `rows.length` on a failure would print `00`, which is a
        // claim ("you have no skills") the server never made.
        action={query.data && query.data.length > 0 ? String(query.data.length).padStart(2, '0') : undefined}
      />
      <QueryState
        query={query}
        errorTitle="Could not load your assistant's skills"
        errorVariant="compact"
        skeleton={
          <ul aria-hidden className="divide-y divide-line rounded-lg border border-line">
            {[0, 1, 2].map((i) => (
              <li key={i} className="flex items-center gap-2 px-3 py-3">
                <Skeleton className="h-3 w-28 rounded-full" delay={i * 0.12} />
                <Skeleton className="h-2.5 w-44 rounded-full" delay={i * 0.12 + 0.12} />
              </li>
            ))}
          </ul>
        }
        empty={<EmptyState icon="✦" title="No skills yet" hint="Teach it its first playbook below." />}
      >
        {(skills) => (
          <ul className="divide-y divide-line rounded-lg border border-line">
            {skills.map((s) => (
              <li key={s.name}>
                <button
                  type="button"
                  onClick={() => setOpen(s.name)}
                  className="flex w-full items-baseline gap-2 px-3 py-2 text-left transition-colors hover:bg-hover"
                >
                  <span className="text-sm text-fg">{s.name}</span>
                  <span className="min-w-0 flex-1 truncate text-xs text-muted">{s.description}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </QueryState>
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
      {error && <p className="text-xs text-danger">{error}</p>}
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
  // This used to hand back whatever the body was, status ignored: a 500 whose
  // body is `{ error }` became `{ content: undefined }` and a non-JSON body
  // became `{ content: '' }` — either way the editor mounted EMPTY, and the
  // first Ctrl+S wrote that emptiness over the real SKILL.md.
  const query = useQuery({
    queryKey: ['assistant-skill', assistant.slug, name],
    queryFn: (): Promise<{ content: string }> => getJson<{ content: string }>(`/api/skills/${assistant.slug}/${name}`),
  })
  const { data } = query

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

  // The editor seeds once from `value`, so it can't mount before the content
  // arrives — but the click must land NOW: show the same modal shell with a
  // prose-bar body, then swap in the real editor.
  if (data === undefined)
    return (
      <Modal open onClose={onClose} title={`${name} · SKILL.md`} width="max-w-6xl">
        <div className="h-[76vh] pt-2">
          {query.isError ? (
            <QueryError error={query.error} title={`Could not open ${name}`} onRetry={() => void query.refetch()} />
          ) : (
            <SkeletonRows rows={8} />
          )}
        </div>
      </Modal>
    )
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
  const query = useQuery({
    queryKey: ['assistant-memory', assistant.id],
    // The old shape leaned on the server's `{ error }` field surviving inside a
    // non-2xx body. A non-2xx WITHOUT that field (a 502 from a proxy, `{}`) fell
    // through as `{ content: undefined }` and drew "Nothing remembered yet" —
    // an emptiness claim about a document nobody actually read.
    queryFn: async (): Promise<{ content: string }> => ({
      content: (await getJson<{ content?: string }>(`/api/memory/${assistant.id}`)).content ?? '',
    }),
  })
  const { data, isLoading } = query
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

  if (isLoading)
    return (
      <div className="space-y-4">
        <SectionHeader
          className="mb-0"
          title="Memory"
          info="What it remembers about you and your work. It updates this itself as you go, and you can edit or prune it any time. Every save is snapshotted, so nothing is ever lost."
        />
        <div aria-hidden className="max-h-72 rounded-lg border border-line p-3">
          <SkeletonRows rows={8} />
        </div>
      </div>
    )
  // A stopped assistant is an explainable state, not a fault — keep its own
  // sentence. Anything else shows the server's reason.
  if (query.isError && data === undefined)
    return assistant.running ? (
      <QueryError error={query.error} title="Memory unavailable" onRetry={() => void query.refetch()} />
    ) : (
      <EmptyState icon="◌" title="Memory unavailable" hint="Start your assistant to read its memory." />
    )

  return (
    <div className="space-y-4">
      <SectionHeader
        className="mb-0"
        title="Memory"
        info="What it remembers about you and your work. It updates this itself as you go, and you can edit or prune it any time. Every save is snapshotted, so nothing is ever lost."
      />
      {data?.content ? (
        <div className="max-h-72 overflow-y-auto rounded-lg border border-line p-3 text-sm">
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
