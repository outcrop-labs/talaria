import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Loader2, Plus, Trash2 } from 'lucide-react'
import { Modal } from '@/components/ui/modal'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { EmptyState } from '@/components/ui/empty-state'
import { cn } from '@/lib/cn'
import { HANDLE_RE, updateAssistant, type Assistant } from '@/lib/assistant'

// Deeper controls for the personal assistant — the member-friendly cut of the
// admin manage modal. General (handle, model tier, on/off), Skills (live
// SKILL.md edits), Memory (what it remembers). All server calls are
// owner-scoped; nothing here needs the admin role.
const TABS = ['General', 'Skills', 'Memory'] as const
type Tab = (typeof TABS)[number]

export function AssistantManageModal({ assistant, onClose }: { assistant: Assistant; onClose: () => void }) {
  const [tab, setTab] = useState<Tab>('General')
  return (
    <Modal open onClose={onClose} title={`Manage ${assistant.displayName}`} width="max-w-xl">
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
        {tab === 'General' && <GeneralTab assistant={assistant} />}
        {tab === 'Skills' && <SkillsTab assistant={assistant} />}
        {tab === 'Memory' && <MemoryTab assistant={assistant} />}
      </div>
    </Modal>
  )
}

function GeneralTab({ assistant }: { assistant: Assistant }) {
  const qc = useQueryClient()
  const [handle, setHandle] = useState(assistant.slug)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const handleOk = HANDLE_RE.test(handle)

  const refresh = async () => qc.invalidateQueries({ queryKey: ['my-assistant'] })

  const run = async (label: string, fn: () => Promise<{ error?: string } | Response>) => {
    setBusy(label)
    setError(null)
    setNote(null)
    try {
      const r = await fn()
      const err = r instanceof Response ? ((await r.json().catch(() => ({}))) as { error?: string }).error : r.error
      if (err) setError(err)
      else {
        await refresh()
        setNote('Done — changes are live.')
      }
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="space-y-5">
      <div>
        <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Handle</label>
        <div className="flex items-center gap-2">
          <span className="text-sm text-muted">@</span>
          <Input value={handle} onChange={(e) => setHandle(e.target.value.toLowerCase())} maxLength={30} />
          <Button
            size="sm"
            disabled={!handleOk || handle === assistant.slug || !!busy}
            onClick={() => void run('handle', () => updateAssistant({ handle }))}
          >
            {busy === 'handle' ? 'Renaming…' : 'Rename'}
          </Button>
        </div>
        <p className={cn('mt-1 text-xs', handle && !handleOk ? 'text-[color:var(--theme-danger)]' : 'text-muted')}>
          {handle && !handleOk
            ? 'Lowercase letters and numbers only, starting with a letter (2–30 characters).'
            : 'Chats, memory, and access move with it — mentions and integrations pick up the new handle.'}
        </p>
      </div>

      {assistant.tiers.length > 0 && (
        <div>
          <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Model</label>
          <div className="space-y-1.5">
            {assistant.tiers.map((t) => (
              <label
                key={t.name}
                className={cn(
                  'flex cursor-pointer items-center gap-2.5 rounded-lg border px-3 py-2 text-sm transition-colors',
                  t.active ? 'border-[var(--theme-accent)]' : 'border-line-subtle hover:border-line',
                )}
              >
                <input
                  type="radio"
                  name="tier"
                  className="accent-[var(--theme-accent)]"
                  checked={t.active}
                  disabled={!!busy}
                  onChange={() => void run('model', () => updateAssistant({ model: t.name }))}
                />
                <span className="capitalize text-fg">{t.name}</span>
                <span className="ml-auto truncate text-xs text-muted">{t.model}</span>
              </label>
            ))}
          </div>
          <p className="mt-1 text-xs text-muted">Which model powers it by default. Switching applies right away.</p>
        </div>
      )}

      <div className="flex items-center gap-3 border-t border-line-subtle pt-4">
        <span
          className="h-2 w-2 rounded-full"
          style={{ background: assistant.running ? 'var(--theme-success)' : 'var(--theme-line)' }}
        />
        <span className="text-sm text-muted">{assistant.running ? 'Running' : 'Stopped'}</span>
        <Button
          variant="outline"
          size="sm"
          className="ml-auto"
          disabled={!!busy}
          onClick={() =>
            void run('power', () =>
              fetch(`/api/fleet/agents/${assistant.id}/control`, {
                method: 'POST',
                credentials: 'same-origin',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ action: assistant.running ? 'stop' : 'up' }),
              }),
            )
          }
        >
          {busy === 'power' ? <Loader2 size={14} className="animate-spin" /> : assistant.running ? 'Stop' : 'Start'}
        </Button>
      </div>

      {note && <p className="text-xs text-[color:var(--theme-success)]">{note}</p>}
      {error && <p className="text-xs text-[color:var(--theme-danger)]">{error}</p>}
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
1. …
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
  const [content, setContent] = useState('')
  const [newName, setNewName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const refresh = async () => qc.invalidateQueries({ queryKey: ['assistant-skills', assistant.slug] })

  const openSkill = async (name: string) => {
    setError(null)
    const r = await fetch(`/api/skills/${assistant.slug}/${name}`, { credentials: 'same-origin' })
    const j = (await r.json().catch(() => null)) as { content?: string; error?: string } | null
    if (!r.ok || j?.content === undefined) return setError(j?.error ?? 'could not open the skill')
    setOpen(name)
    setContent(j.content)
  }

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

  const remove = async (name: string) => {
    if (!confirm(`Remove the "${name}" skill?`)) return
    setBusy(true)
    try {
      await fetch(`/api/skills/${assistant.slug}/${name}`, { method: 'DELETE', credentials: 'same-origin' })
      setOpen(null)
      await refresh()
    } finally {
      setBusy(false)
    }
  }

  if (open !== null) {
    return (
      <div className="space-y-3">
        <div className="flex items-center gap-2">
          <button type="button" className="text-xs text-muted hover:text-fg" onClick={() => setOpen(null)}>
            ← Skills
          </button>
          <span className="text-sm font-medium text-fg">{open}</span>
          <button
            type="button"
            title="Remove skill"
            className="ml-auto text-muted hover:text-[color:var(--theme-danger)]"
            onClick={() => void remove(open)}
          >
            <Trash2 size={15} />
          </button>
        </div>
        <Textarea rows={12} value={content} onChange={(e) => setContent(e.target.value)} className="font-[var(--font-mono)] text-xs" />
        <div className="flex items-center gap-3">
          <Button size="sm" disabled={busy} onClick={() => void save(open, content)}>
            {busy ? 'Saving…' : 'Save'}
          </Button>
          <span className="text-xs text-muted">Skills are live — no restart needed.</span>
        </div>
        {error && <p className="text-xs text-[color:var(--theme-danger)]">{error}</p>}
      </div>
    )
  }

  return (
    <div className="space-y-3">
      <p className="text-xs text-muted">
        Skills are step-by-step playbooks your assistant follows for recurring jobs — weekly summaries, travel
        planning, whatever you teach it.
      </p>
      {isLoading ? null : skills.length === 0 ? (
        <EmptyState icon="✦" title="No skills yet" hint="Teach it its first playbook below." />
      ) : (
        <ul className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
          {skills.map((s) => (
            <li key={s.name}>
              <button
                type="button"
                onClick={() => void openSkill(s.name)}
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
              setNewName('')
              void openSkill(newName)
            })
          }
        >
          <Plus size={14} /> Add
        </Button>
      </div>
      {error && <p className="text-xs text-[color:var(--theme-danger)]">{error}</p>}
    </div>
  )
}

function MemoryTab({ assistant }: { assistant: Assistant }) {
  const { data, isLoading } = useQuery({
    queryKey: ['assistant-memory', assistant.id],
    queryFn: async (): Promise<{ content: string; error?: string }> => {
      const r = await fetch(`/api/memory/${assistant.id}`, { credentials: 'same-origin' })
      return ((await r.json().catch(() => null)) as { content: string; error?: string } | null) ?? { content: '', error: 'could not load memory' }
    },
  })
  const [content, setContent] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null)

  useEffect(() => {
    if (data && content === null && data.error === undefined) setContent(data.content)
  }, [data, content])

  const save = async () => {
    if (content === null) return
    setBusy(true)
    setMsg(null)
    try {
      const r = await fetch(`/api/memory/${assistant.id}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content }),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) setMsg({ ok: false, text: j.error ?? 'could not save' })
      else setMsg({ ok: true, text: 'Saved.' })
    } finally {
      setBusy(false)
    }
  }

  if (isLoading) return <div className="text-sm text-muted">Loading…</div>
  if (data?.error)
    return <EmptyState icon="◌" title="Memory unavailable" hint={assistant.running ? data.error : 'Start your assistant to read its memory.'} />

  return (
    <div className="space-y-3">
      <p className="text-xs text-muted">
        What it remembers about you and your work — it updates this itself as you go, and you can edit or prune it any
        time.
      </p>
      <Textarea rows={12} value={content ?? ''} onChange={(e) => setContent(e.target.value)} className="font-[var(--font-mono)] text-xs" />
      <div className="flex items-center gap-3">
        <Button size="sm" disabled={busy || content === null} onClick={() => void save()}>
          {busy ? 'Saving…' : 'Save'}
        </Button>
        {msg && (
          <span className={cn('text-xs', msg.ok ? 'text-[color:var(--theme-success)]' : 'text-[color:var(--theme-danger)]')}>
            {msg.text}
          </span>
        )}
      </div>
    </div>
  )
}
