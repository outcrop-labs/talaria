import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { stringify as stringifyYaml } from 'yaml'
import { Loader2, Check, Lock, X, RotateCcw, Plug } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { confirm } from '@/components/ui/confirm'
import { Input } from '@/components/ui/input'
import { submitOnEnter } from '@/components/ui/control'
import { Modal } from '@/components/ui/modal'
import { Generating } from '@/components/ui/generating'
import { Select } from '@/components/ui/select'
import { Markdown } from '@/components/ui/markdown'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { InfoTip } from '@/components/ui/info-tip'
import { cn } from '@/lib/cn'
import { relativeTime, useFleet } from '@/lib/fleet'
import { Segmented } from '@/components/ui/segmented'
import { Tabs } from '@/components/ui/tabs'
import { Chip } from '@/components/ui/chip'
import { useModels } from '@/lib/muse'
import { AgentConfigForm } from '@/components/fleet/agent-editor'
import { CronsPanel } from '@/components/fleet/agent-crons'
import { SecretsTab } from '@/components/fleet/agent-secrets-tab'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { patchAgentMeta, type AgentDef, type LlmEndpoint, type ModelTarget } from '@/lib/fleet-defs'
import { useTemplates } from '@/lib/templates'

type Tab = 'summary' | 'config' | 'skills' | 'memory' | 'crons' | 'secrets' | 'mcp' | 'versions'
const TABS: { id: Tab; label: string }[] = [
  { id: 'summary', label: 'Summary' },
  { id: 'config', label: 'Config' },
  { id: 'skills', label: 'Skills' },
  { id: 'memory', label: 'Memory' },
  { id: 'crons', label: 'Crons' },
  { id: 'secrets', label: 'Secrets' },
  { id: 'mcp', label: 'MCP' },
  { id: 'versions', label: 'Versions' },
]

// The whole of an agent's internal stack in one modal: model config, skills,
// memory, MCP servers, and version history — no more hopping between top-level
// pages. Read-only for non-admins.
export function AgentManageModal({
  open,
  onClose,
  def,
  endpoints,
  isAdmin,
}: {
  open: boolean
  onClose: () => void
  def: AgentDef
  endpoints: LlmEndpoint[]
  isAdmin: boolean
}) {
  const [tab, setTab] = useState<Tab>('summary')

  return (
    <Modal open={open} onClose={onClose} title={`${def.displayName} · v${def.currentVersion}`} takeover>
      {/* Takeover modal: the frame owns the height, so every tab renders at
          the SAME size and scrolls internally — no height jank between tabs. */}
      <div className="flex h-full min-h-0 flex-col">
        <Tabs items={TABS} value={tab} onChange={setTab} className="shrink-0 border-b border-line pb-2" />

        <div className="min-h-0 flex-1 overflow-y-auto pt-4">
          {tab === 'summary' && <SummaryTab def={def} isAdmin={isAdmin} />}
          {tab === 'config' &&
            (isAdmin ? (
              <AgentConfigForm def={def} endpoints={endpoints} onSaved={onClose} />
            ) : (
              <ReadOnlyConfig def={def} />
            ))}
          {tab === 'skills' && <SkillsTab slug={def.slug} isAdmin={isAdmin} />}
          {tab === 'memory' && <MemoryTab def={def} isAdmin={isAdmin} />}
          {tab === 'crons' && <CronsPanel agentId={def.id} />}
          {tab === 'secrets' && (isAdmin ? <SecretsTab agentId={def.id} /> : <div className="text-sm text-muted">Admins only.</div>)}
          {tab === 'mcp' && <McpTab def={def} isAdmin={isAdmin} />}
          {tab === 'versions' && <VersionsTab def={def} isAdmin={isAdmin} />}
        </div>
      </div>
    </Modal>
  )
}

// ── Summary ───────────────────────────────────────────────────────────────────
function TargetChip({ t, name }: { t: ModelTarget; name?: string }) {
  const local = /inference|vllm|ollama|spark|local/.test(t.endpoint)
  // Model identity is chrome — mono chip, radius 6, class in signal color.
  return (
    <span className="inline-flex items-center gap-1.5 rounded-md border border-line px-2 py-0.5 font-mono text-[10px] tracking-[0.05em]">
      {name && <span className="font-medium uppercase text-fg">{name}</span>}
      <span className="text-muted">{t.model}</span>
      <span className={cn('uppercase', local ? 'text-success' : 'text-accent')}>{local ? 'self-hosted' : 'cloud'}</span>
    </span>
  )
}

function SummaryTab({ def, isAdmin }: { def: AgentDef; isAdmin: boolean }) {
  const qc = useQueryClient()
  const cfg = def.latest?.config
  const { data: fleet, isLoading: fleetLoading } = useFleet()
  const stat = fleet?.agents.find((a) => a.id === def.model)
  const [role, setRole] = useState(def.role ?? '')
  const saveRole = async () => {
    if (role.trim() === (def.role ?? '')) return
    await patchAgentMeta(def.id, { role: role.trim() || null })
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
  return (
    <div className="space-y-4 text-sm">
      {/* Editable role — the human-readable title shown on the roster. */}
      <div>
        <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Role</div>
        {isAdmin ? (
          <Input size="sm" value={role} onChange={(e) => setRole(e.target.value)} onBlur={() => void saveRole()} placeholder="e.g. Support Lead" className="max-w-xs" />
        ) : (
          <div className="text-fg">{def.role ?? '—'}</div>
        )}
      </div>
      {/* Workbench — THE sandbox setting: off / auto (fit rules) / on. */}
      <WorkbenchControl def={def} isAdmin={isAdmin} />
      <div className="grid grid-cols-2 gap-3">
        <Stat label="Model id" value={def.model} />
        <Stat label="Department" value={def.department} />
        <Stat label="Management" value="Talaria-managed" />
        <Stat label="Version" value={`v${def.currentVersion}`} />
        {fleetLoading ? (
          // The usage cells land late and grow the grid — hold their spots.
          <>
            <StatSkeleton />
            <StatSkeleton delay={0.12} />
            <StatSkeleton delay={0.24} />
          </>
        ) : (
          <>
            {stat && <Stat label="Conversations" value={String(stat.conversations)} />}
            {stat && <Stat label="Messages" value={String(stat.messages)} />}
            {stat?.lastUsed && <Stat label="Last used" value={relativeTime(stat.lastUsed)} />}
          </>
        )}
      </div>
      <div>
        <div className="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Models</div>
        <div className="flex flex-wrap items-center gap-2">
          {cfg?.main && <TargetChip t={cfg.main} name="main" />}
          {cfg?.aliases?.map((a) => <TargetChip key={a.name} t={a} name={a.name} />)}
          {!cfg?.main && <span className="text-xs text-muted">no config yet</span>}
        </div>
        {!!cfg?.fallbacks?.length && (
          <div className="mt-2 text-xs text-muted">↯ fallback: {cfg.fallbacks.map((f) => f.model).join(' → ')}</div>
        )}
      </div>
      {!!cfg?.mcpServers?.length && (
        <Stat label="MCP" value={cfg.mcpServers.join(', ')} />
      )}
      <TemplateBindings def={def} isAdmin={isAdmin} />
    </div>
  )
}

// This agent's template overrides — they beat the board default wherever the
// agent drafts or creates tickets, and shape its plan documents.
function TemplateBindings({ def, isAdmin }: { def: AgentDef; isAdmin: boolean }) {
  const qc = useQueryClient()
  const { data: templates = [], isLoading } = useTemplates()
  const nameOf = (id: string | null) => templates.find((t) => t.id === id)?.name ?? '—'
  const bind = async (patch: { ticketTemplateId?: string | null; planTemplateId?: string | null }) => {
    await patchAgentMeta(def.id, patch)
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
  const pick = (kind: 'ticket' | 'plan', value: string | null, onChange: (v: string | null) => void) => (
    <Select value={value ?? ''} size="sm" onChange={(e) => onChange(e.target.value || null)} className="w-full">
      <option value="">{kind === 'ticket' ? 'Board default' : 'Freeform'}</option>
      {templates
        .filter((t) => t.kind === kind)
        .map((t) => (
          <option key={t.id} value={t.id}>
            {t.name}
          </option>
        ))}
    </Select>
  )
  return (
    <div>
      <div className="mb-1.5 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Templates
        <InfoTip text="A bound template wins over the board's default wherever this agent writes tickets or plans." />
      </div>
      {isLoading ? (
        // Until templates land the selects would show the fallback option and
        // then flip — hold both with select-shaped shimmer instead.
        <div className="grid grid-cols-2 gap-3">
          <div>
            <div className="mb-1 text-[11px] text-muted">Tickets</div>
            <Skeleton className="h-9 w-full" />
          </div>
          <div>
            <div className="mb-1 text-[11px] text-muted">Plan documents</div>
            <Skeleton className="h-9 w-full" delay={0.12} />
          </div>
        </div>
      ) : isAdmin ? (
        <div className="grid grid-cols-2 gap-3">
          <div>
            <div className="mb-1 text-[11px] text-muted">Tickets</div>
            {pick('ticket', def.ticketTemplateId, (v) => void bind({ ticketTemplateId: v }))}
          </div>
          <div>
            <div className="mb-1 text-[11px] text-muted">Plan documents</div>
            {pick('plan', def.planTemplateId, (v) => void bind({ planTemplateId: v }))}
          </div>
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-3">
          <Stat label="Tickets" value={nameOf(def.ticketTemplateId)} />
          <Stat label="Plan documents" value={nameOf(def.planTemplateId)} />
        </div>
      )}
    </div>
  )
}
// §8 stat cell: 10px mono uppercase ink-dim label over a readout value.
const Stat = ({ label, value }: { label: string; value: string }) => (
  <div>
    <div className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</div>
    <div className="truncate text-fg" title={value}>{value}</div>
  </div>
)
/** A Stat cell's shape while its query is in flight: label bar + value bar. */
const StatSkeleton = ({ delay = 0 }: { delay?: number }) => (
  <div className="space-y-1.5">
    <Skeleton className="h-2.5 w-20 rounded-full" delay={delay} />
    <Skeleton className="h-3.5 w-24 rounded-full" delay={delay + 0.12} />
  </div>
)

function ReadOnlyConfig({ def }: { def: AgentDef }) {
  const cfg = def.latest?.config
  return (
    <div className="space-y-3 text-sm">
      <Field label="Main model" value={cfg?.main ? `${cfg.main.endpoint} · ${cfg.main.model}` : '—'} />
      <Field label="Tiers" value={cfg?.aliases?.map((a) => a.name).join(', ') || '—'} />
      <Field label="Fallbacks" value={cfg?.fallbacks?.length ? String(cfg.fallbacks.length) : 'none'} />
      {def.latest?.soul && (
        <div>
          <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Soul</div>
          <div className="max-h-64 overflow-y-auto rounded-lg border border-line p-3 text-xs">
            <Markdown>{def.latest.soul}</Markdown>
          </div>
        </div>
      )}
    </div>
  )
}
const Field = ({ label, value }: { label: string; value: string }) => (
  <div className="flex gap-3">
    <span className="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</span>
    <span className="min-w-0 flex-1 text-fg">{value}</span>
  </div>
)

// ── Skills ────────────────────────────────────────────────────────────────────
interface SkillSummary {
  name: string
  description: string
  files: string[]
}
function SkillsTab({ slug, isAdmin }: { slug: string; isAdmin: boolean }) {
  const qc = useQueryClient()
  const { data, isLoading } = useQuery({
    queryKey: ['skills'],
    queryFn: async () => {
      const r = await fetch('/api/skills')
      if (!r.ok) throw new Error('failed')
      return ((await r.json()) as { owners: Array<{ owner: string; skills: SkillSummary[] }> }).owners
    },
  })
  const skills = data?.find((o) => o.owner === slug)?.skills ?? []
  const [open, setOpen] = useState<string | null>(null)
  const [newName, setNewName] = useState('')

  const create = async () => {
    const name = newName.trim().toLowerCase().replace(/\s+/g, '-')
    if (!name) return
    await fetch(`/api/skills/${slug}/${name}`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content: `# ${name}\n\ndescription: what this skill is for\n\n## Steps\n\n1. \n` }),
    })
    setNewName('')
    await qc.invalidateQueries({ queryKey: ['skills'] })
    setOpen(name)
  }

  return (
    <div className="space-y-3">
      {isLoading ? (
        <SkeletonRows rows={3} className="py-2" />
      ) : skills.length === 0 ? (
        <EmptyState icon="✦" title="No skills yet" hint={isAdmin ? 'Add one below.' : undefined} />
      ) : (
        <div className="divide-y divide-line">
          {skills.map((s) => (
            <button key={s.name} type="button" onClick={() => setOpen(s.name)} className="flex w-full items-baseline gap-3 py-2.5 text-left transition-colors hover:bg-hover">
              <span className="shrink-0 font-sans text-sm font-medium text-fg">{s.name}</span>
              <span className="min-w-0 truncate font-sans text-sm text-muted">{s.description}</span>
              {s.files.length > 1 && <span className="ml-auto shrink-0 font-mono text-[11px] text-muted">{s.files.length} files</span>}
            </button>
          ))}
        </div>
      )}
      {isAdmin && (
        <div className="flex items-center gap-2 pt-1">
          <Input size="sm" value={newName} onChange={(e) => setNewName(e.target.value)} placeholder="new-skill-name" className="w-52" onKeyDown={(e) => e.key === 'Enter' && void create()} />
          <Button size="sm" onClick={() => void create()} disabled={!newName.trim()}>
            Add skill
          </Button>
        </div>
      )}
      {open && <SkillEditorModal slug={slug} name={open} isAdmin={isAdmin} onClose={() => setOpen(null)} />}
    </div>
  )
}

function SkillEditorModal({ slug, name, isAdmin, onClose }: { slug: string; name: string; isAdmin: boolean; onClose: () => void }) {
  const qc = useQueryClient()
  const { data } = useQuery({
    queryKey: ['skill', slug, name],
    queryFn: async () => {
      const r = await fetch(`/api/skills/${slug}/${name}`)
      if (!r.ok) throw new Error('failed')
      return (await r.json()) as { content: string; files: string[] }
    },
  })
  const [busy, setBusy] = useState(false)
  const save = async (content: string) => {
    setBusy(true)
    try {
      await fetch(`/api/skills/${slug}/${name}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ content }) })
      await qc.invalidateQueries({ queryKey: ['skills'] })
      await qc.invalidateQueries({ queryKey: ['skill', slug, name] })
    } finally {
      setBusy(false)
    }
  }
  const remove = async () => {
    if (!(await confirm({ title: 'Delete skill', message: `Delete the "${name}" skill?`, confirmLabel: 'Delete', danger: true }))) return
    await fetch(`/api/skills/${slug}/${name}`, { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['skills'] })
    onClose()
  }
  // Don't mount the editor until the content is here — it seeds ONCE from
  // `value`; mounting during the fetch would show an empty skill forever.
  // But the CLICK must land instantly: slide in the editor-shaped shell
  // (same surface InternalEditorModal uses in nested mode) with a skeleton
  // body, then swap to the real editor when the content arrives.
  if (!data)
    return (
      <div className="absolute inset-0 z-30 flex flex-col bg-[var(--theme-panel)]">
        <div className="flex shrink-0 items-center gap-2 border-b border-line-subtle px-6 py-3.5">
          <div className="text-sm font-semibold text-fg">{name} · SKILL.md</div>
        </div>
        <div className="space-y-3 p-6" aria-hidden>
          <Skeleton className="h-2.5 w-2/3 rounded-full" />
          <Skeleton className="h-2.5 w-full rounded-full" delay={0.12} />
          <Skeleton className="h-2.5 w-5/6 rounded-full" delay={0.24} />
          <Skeleton className="h-2.5 w-3/4 rounded-full" delay={0.36} />
          <Skeleton className="h-2.5 w-1/2 rounded-full" delay={0.48} />
        </div>
      </div>
    )
  return (
    <InternalEditorModal
      open
      nested
      onClose={onClose}
      title={`${name} · SKILL.md`}
      subtitle="Read live. The agent picks up edits on its next run."
      value={data.content}
      editable={isAdmin}
      saving={busy}
      onSave={save}
      history={{ kind: 'skill', owner: slug, name }}
      muse={{ kind: 'skill', context: `A skill for the "${slug}" agent.` }}
      footerExtra={isAdmin ? <Button variant="ghost" size="sm" onClick={() => void remove()}>Delete skill</Button> : undefined}
    />
  )
}

// ── Memory ────────────────────────────────────────────────────────────────────
function MemoryTab({ def, isAdmin }: { def: AgentDef; isAdmin: boolean }) {
  const qc = useQueryClient()
  const { data, error, isLoading } = useQuery({
    queryKey: ['memory', def.id],
    queryFn: async () => {
      const r = await fetch(`/api/memory/${def.id}`)
      const j = (await r.json()) as { content?: string; error?: string; container?: string }
      if (!r.ok || j.error) throw new Error(j.error ?? 'failed')
      return { content: j.content ?? '', container: j.container ?? '' }
    },
    retry: false,
  })
  const [editing, setEditing] = useState(false)
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState('')
  const save = async (content: string) => {
    setBusy(true)
    try {
      const r = await fetch(`/api/memory/${def.id}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ content }) })
      const j = (await r.json()) as { error?: string }
      if (!j.error) await qc.invalidateQueries({ queryKey: ['memory', def.id] })
    } finally {
      setBusy(false)
    }
  }
  // Quick-add: append a dated note without opening the full editor.
  const addNote = async () => {
    const n = note.trim()
    if (!n || isLoading) return // appending before the fetch lands would clobber the doc
    const stamp = new Date().toISOString().slice(0, 10)
    const base = (data?.content ?? '').replace(/\s+$/, '')
    await save(`${base ? `${base}\n` : ''}- ${n} _(added by hand, ${stamp})_\n`)
    setNote('')
  }
  if (!def.managed) return <EmptyState icon="❖" title="Not managed" hint="Memory reads through the managed container. Migrate this agent first." />
  if (error) return <EmptyState icon="❖" title="Can't reach the agent" hint={(error as Error).message} />
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-1.5">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Memory</span>
        <InfoTip text={`Lives in the agent's own container (${data?.container ?? ''}) — the agent curates it too, so a concurrent agent write can win over a manual edit. Every save is snapshotted and revertible.`} />
        {isAdmin && (
          <Button size="sm" className="ml-auto shrink-0" disabled={isLoading} onClick={() => setEditing(true)}>
            Edit
          </Button>
        )}
      </div>
      {isAdmin && (
        <div className="flex items-center gap-2">
          <Input
            size="sm"
            value={note}
            onChange={(e) => setNote(e.target.value)}
            onKeyDown={submitOnEnter(() => void addNote())}
            placeholder="Add a memory — one fact the agent should keep"
            className="flex-1"
          />
          <Button size="sm" variant="outline" disabled={busy || isLoading || !note.trim()} onClick={() => void addNote()}>
            Add
          </Button>
        </div>
      )}
      {isLoading ? (
        // The document is coming — prose-bar shimmer where it will render.
        <div className="space-y-3 pt-1" aria-hidden>
          <Skeleton className="h-2.5 w-3/4 rounded-full" />
          <Skeleton className="h-2.5 w-full rounded-full" delay={0.12} />
          <Skeleton className="h-2.5 w-5/6 rounded-full" delay={0.24} />
          <Skeleton className="h-2.5 w-2/3 rounded-full" delay={0.36} />
          <Skeleton className="h-2.5 w-full rounded-full" delay={0.48} />
          <Skeleton className="h-2.5 w-1/2 rounded-full" delay={0.6} />
        </div>
      ) : data?.content ? (
        <div className="text-sm">
          <Markdown>{data.content}</Markdown>
        </div>
      ) : (
        <EmptyState icon="❖" title="No memory yet" hint="The agent hasn't written anything down." />
      )}
      {editing && (
        <InternalEditorModal
          open
          nested
          onClose={() => setEditing(false)}
          title={`${def.displayName} · MEMORY.md`}
          subtitle="The agent maintains this itself; your edits are snapshotted and revertible."
          value={data?.content ?? ''}
          editable={isAdmin}
          saving={busy}
          onSave={save}
          history={{ kind: 'memory', id: def.id }}
          muse={{ kind: 'memory', context: `The memory of the "${def.slug}" agent (${def.role ?? def.department}).` }}
        />
      )}
    </div>
  )
}

// ── MCP ───────────────────────────────────────────────────────────────────────
type ProbeState = 'ok' | 'auth' | 'unreachable' | 'error'
interface Probe {
  state: ProbeState
  detail: string
}
const PROBE_UI: Record<ProbeState, { color: string; icon: React.ReactNode; label: string }> = {
  ok: { color: 'var(--theme-success)', icon: <Check size={13} />, label: 'Connected' },
  auth: { color: 'var(--theme-warning)', icon: <Lock size={13} />, label: 'Login required' },
  unreachable: { color: 'var(--theme-danger)', icon: <X size={13} />, label: 'Unreachable' },
  error: { color: 'var(--theme-danger)', icon: <X size={13} />, label: 'Error' },
}
interface McpServer {
  name: string
  url: string
  timeout: number | null
  extras: string[]
}
function McpTab({ def, isAdmin }: { def: AgentDef; isAdmin: boolean }) {
  const qc = useQueryClient()
  const { data, isLoading } = useQuery({
    queryKey: ['mcp-agents'],
    queryFn: async () => {
      const r = await fetch('/api/mcp')
      if (!r.ok) throw new Error('failed')
      return ((await r.json()) as { agents: Array<{ id: string; servers: McpServer[] }> }).agents
    },
  })
  const servers = data?.find((a) => a.id === def.id)?.servers ?? []
  const [probes, setProbes] = useState<Record<string, Probe | 'testing'>>({})
  const [name, setName] = useState('')
  const [url, setUrl] = useState('')
  const [busy, setBusy] = useState(false)

  const test = async (s: McpServer) => {
    setProbes((p) => ({ ...p, [s.name]: 'testing' }))
    const r = await fetch('/api/mcp/test', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ url: s.url, agentSlug: def.slug }) }).catch(() => null)
    const result: Probe = r?.ok ? ((await r.json()) as Probe) : { state: 'error', detail: 'test failed' }
    setProbes((p) => ({ ...p, [s.name]: result }))
  }
  const edit = async (body: { add?: Array<{ name: string; url: string }>; remove?: string[] }) => {
    setBusy(true)
    try {
      await fetch(`/api/fleet/defs/${def.id}/mcp`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ add: [], remove: [], apply: true, ...body }) })
      setName('')
      setUrl('')
      await qc.invalidateQueries({ queryKey: ['mcp-agents'] })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3">
      {busy && (
        <Generating label={`Applying MCP change: rolling ${def.displayName} so the new version takes effect`} lines={2} />
      )}
      {isLoading ? (
        <SkeletonRows rows={2} className="py-2" />
      ) : servers.length === 0 ? (
        <div className="text-sm text-muted">No MCP servers connected.</div>
      ) : (
        <div className="divide-y divide-line">
          {servers.map((s) => {
            const probe = probes[s.name]
            return (
              <div key={s.name} className="flex items-center gap-3 py-2.5 text-sm">
                <span className="w-28 shrink-0 truncate font-medium text-fg">{s.name}</span>
                {s.extras.includes('built-in') && (
                  <Chip className="shrink-0" title="The Talaria toolkit, attached to every managed agent automatically">
                    built-in
                  </Chip>
                )}
                {s.extras.includes('managed') && (
                  <Link
                    to="/mcp"
                    className="shrink-0 rounded border border-accent/50 bg-accent/10 px-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-accent transition-colors hover:bg-accent/15"
                    title="Attached from the org MCP registry — assignment, tool subsets, and credentials are managed on the MCP page"
                  >
                    org registry
                  </Link>
                )}
                <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted">{s.url}</span>
                {probe === 'testing' ? (
                  <Loader2 size={13} className="shrink-0 animate-spin text-muted" />
                ) : probe ? (
                  <span className="flex shrink-0 items-center gap-1 text-xs" style={{ color: PROBE_UI[probe.state].color }} title={probe.detail}>
                    {PROBE_UI[probe.state].icon} {PROBE_UI[probe.state].label}
                  </span>
                ) : null}
                {!s.extras.includes('managed') && (
                  <button type="button" onClick={() => void test(s)} className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-accent">
                    Test
                  </button>
                )}
                {isAdmin && !s.extras.includes('built-in') && !s.extras.includes('managed') && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={async () => { if (await confirm({ title: 'Remove skill', message: `Remove "${s.name}"?`, confirmLabel: 'Remove', danger: true })) void edit({ remove: [s.name] }) }}
                    className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-danger"
                  >
                    Remove
                  </button>
                )}
              </div>
            )
          })}
        </div>
      )}
      {isAdmin && (
        <div className="flex items-center gap-2 pt-1">
          <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={submitOnEnter(() => !busy && name.trim() && /^https?:\/\//.test(url.trim()) && void edit({ add: [{ name: name.trim(), url: url.trim() }] }))} placeholder="name" className="w-28" />
          <Input size="sm" value={url} onChange={(e) => setUrl(e.target.value)} onKeyDown={submitOnEnter(() => !busy && name.trim() && /^https?:\/\//.test(url.trim()) && void edit({ add: [{ name: name.trim(), url: url.trim() }] }))} placeholder="http://host:port/mcp" className="flex-1" />
          <Button size="sm" disabled={busy || !name.trim() || !/^https?:\/\//.test(url.trim())} onClick={() => void edit({ add: [{ name: name.trim(), url: url.trim() }] })}>
            <Plug size={14} className="mr-1.5" /> Add
          </Button>
        </div>
      )}
    </div>
  )
}

// ── Versions ──────────────────────────────────────────────────────────────────
interface Version {
  version: number
  note: string | null
  createdBy: string | null
  createdAt: string
}
function VersionsTab({ def, isAdmin }: { def: AgentDef; isAdmin: boolean }) {
  const qc = useQueryClient()
  const { data, isLoading } = useQuery({
    queryKey: ['agent-versions', def.id],
    queryFn: async () => {
      const r = await fetch(`/api/fleet/defs/${def.id}/versions`)
      if (!r.ok) throw new Error('failed')
      return ((await r.json()) as { versions: Version[] }).versions
    },
  })
  const [busy, setBusy] = useState<number | null>(null)
  const [configOpen, setConfigOpen] = useState(false)
  const revert = async (v: number) => {
    if (!(await confirm({ title: 'Revert version', message: `Revert ${def.displayName} to v${v}? This publishes it as a new version.`, confirmLabel: 'Revert' }))) return
    setBusy(v)
    try {
      await fetch(`/api/fleet/defs/${def.id}/versions`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ revertTo: v }) })
      await qc.invalidateQueries({ queryKey: ['agent-versions', def.id] })
      await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    } finally {
      setBusy(null)
    }
  }
  const versions = data ?? []
  // The header row (History + Config history) renders in every state — only
  // the list below it swaps between skeleton, empty, and rows.
  return (
    <div className="divide-y divide-line">
      {busy !== null && (
        <div className="pb-2.5">
          <Generating label={`Reverting to v${busy}, publishing as a new version`} lines={2} />
        </div>
      )}
      <div className="flex items-center gap-1.5 pb-2.5">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">History</span>
        <InfoTip text="Reverting never destroys anything — the old content is published as a NEW version on top." />
        <Button variant="outline" size="sm" className="ml-auto" onClick={() => setConfigOpen(true)}>
          Config history
        </Button>
      </div>
      {configOpen && (
        <InternalEditorModal
          open
          nested
          onClose={() => setConfigOpen(false)}
          title={`${def.displayName} · config`}
          subtitle="The rendered model/tool config per version. Click a revision to see what changed."
          value={stringifyYaml(def.latest?.config ?? {})}
          editable={false}
          onSave={() => {}}
          history={{ kind: 'config', id: def.id }}
          mode="plain"
        />
      )}
      {isLoading && <SkeletonRows rows={4} className="py-3" />}
      {!isLoading && versions.length === 0 && <div className="py-2.5 text-sm text-muted">No version history.</div>}
      {versions.map((v) => (
        <div key={v.version} className="flex items-center gap-3 py-2.5 text-sm transition-colors hover:bg-hover">
          <span className={cn('w-12 shrink-0 font-mono', v.version === def.currentVersion ? 'text-accent' : 'text-muted')}>v{v.version}</span>
          <span className="min-w-0 flex-1 truncate font-sans text-fg">{v.note ?? '—'}</span>
          <span className="shrink-0 font-mono text-[11px] text-muted">{v.createdBy ?? 'system'} · {relativeTime(v.createdAt)}</span>
          {isAdmin && v.version !== def.currentVersion && (
            <button type="button" disabled={busy !== null} onClick={() => void revert(v.version)} className="flex shrink-0 items-center gap-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-accent">
              <RotateCcw size={12} /> {busy === v.version ? 'reverting' : 'revert'}
            </button>
          )}
          {v.version === def.currentVersion && <span className="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-success">current</span>}
        </div>
      ))}
    </div>
  )
}


interface WorkbenchProfileLite {
  slug: string
  name: string
  description: string
  harnesses: string[]
  autoAttach: { departments?: string[]; roles?: string[] }
  enabled: boolean
}

const EFFORTS = [
  ['light', 'Low'],
  ['standard', 'Medium'],
  ['heavy', 'High'],
] as const

/** The one workbench setting. Auto shows what it resolves to for THIS agent
 *  (fit by department/role); On lets an admin force a specific profile. */
function WorkbenchControl({ def, isAdmin }: { def: AgentDef; isAdmin: boolean }) {
  const qc = useQueryClient()
  const { data: profiles = [] } = useQuery({
    queryKey: ['workbench-profiles'],
    queryFn: async (): Promise<WorkbenchProfileLite[]> => {
      const r = await fetch('/api/workbench', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { profiles: WorkbenchProfileLite[] }).profiles
    },
  })
  const mode = def.workbench ?? 'auto'
  const fits = (p: WorkbenchProfileLite) =>
    (p.autoAttach.departments ?? []).some((d) => d.toLowerCase() === def.department.toLowerCase()) ||
    (!!def.role && (p.autoAttach.roles ?? []).some((r) => (def.role ?? '').toLowerCase().includes(r.toLowerCase())))
  // Mirrors server resolveWorkbench (enabled profiles only) — keep in sync.
  const live = profiles.filter((p) => p.enabled)
  const resolved =
    mode === 'off'
      ? null
      : mode === 'on'
        ? (live.find((p) => p.slug === def.workbenchProfile) ?? live.find(fits) ?? live.find((p) => p.slug === 'dev') ?? null)
        : (live.find((p) => p.slug === def.workbenchProfile) ?? live.find(fits) ?? null)

  const save = async (patch: { workbench?: 'off' | 'auto' | 'on'; workbenchProfile?: string | null }) => {
    await patchAgentMeta(def.id, patch)
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }

  return (
    <div>
      <div className="mb-1 flex items-center gap-1.5">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Workbench</span>
        <InfoTip text="A sandboxed runtime scoped to this agent's role — tools, harnesses, and creds it can execute real work with. Auto attaches the profile that fits (department/role); On forces one; Off disables it." />
      </div>
      {isAdmin ? (
        <div className="flex flex-wrap items-center gap-2">
          <Segmented
            options={[
              { id: 'off', label: 'Off' },
              { id: 'auto', label: 'Auto' },
              { id: 'on', label: 'On' },
            ]}
            value={mode}
            onChange={(m) => void save({ workbench: m })}
          />
          {mode !== 'off' && profiles.length > 1 && (
            <Select
              size="sm"
              value={def.workbenchProfile ?? ''}
              onChange={(e) => void save({ workbenchProfile: e.target.value || null })}
            >
              <option value="">best fit</option>
              {profiles
                .filter((p) => p.enabled)
                .map((p) => (
                  <option key={p.slug} value={p.slug}>
                    {p.name}
                  </option>
                ))}
            </Select>
          )}
          <span className="font-mono text-[11px] text-muted">
            {mode === 'off' ? 'no sandbox' : resolved ? `→ ${resolved.name}${resolved.harnesses.length ? ` (${resolved.harnesses.join(', ')})` : ''}` : '→ nothing fits — no sandbox'}
          </span>
        </div>
      ) : (
        <div className="text-fg">{mode === 'off' ? 'Off' : resolved ? resolved.name : 'None'}</div>
      )}
      {isAdmin && mode !== 'off' && resolved && (
        <>
          <WorkbenchTuning def={def} profile={resolved} save={save} />
          <WorkbenchRepos agentId={def.id} />
        </>
      )}
    </div>
  )
}

/** The per-agent knobs: WHICH harness this agent runs, and which model each
 *  effort level means for it (blank = the org-wide Workbench roles). */
function WorkbenchTuning({
  def,
  profile,
  save,
}: {
  def: AgentDef
  profile: WorkbenchProfileLite
  save: (patch: Parameters<typeof patchAgentMeta>[1]) => Promise<void>
}) {
  const { data: modelsData } = useModels()
  const models = Array.isArray(modelsData) ? [] : (modelsData?.models ?? [])
  // Labels come from the harness REGISTRY (builtin + app-shipped + custom).
  const { data: registry = [] } = useQuery({
    queryKey: ['workbench-harnesses'],
    queryFn: async (): Promise<Array<{ slug: string; label: string }>> => {
      const r = await fetch('/api/workbench/harnesses', { credentials: 'same-origin' })
      if (!r.ok) return []
      return ((await r.json()) as { harnesses: Array<{ slug: string; label: string }> }).harnesses
    },
  })
  const labelOf = (slug: string) => registry.find((h) => h.slug === slug)?.label ?? slug
  const chosen = def.workbenchHarness && profile.harnesses.includes(def.workbenchHarness) ? def.workbenchHarness : ''
  return (
    <div className="mt-2 space-y-1.5">
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Harness</span>
        <Select size="sm" value={chosen} onChange={(e) => void save({ workbenchHarness: e.target.value || null })} className="w-40">
          <option value="">Auto ({profile.harnesses[0] ? labelOf(profile.harnesses[0]) : 'none'})</option>
          {profile.harnesses.map((h) => (
            <option key={h} value={h}>
              {labelOf(h)}
            </option>
          ))}
        </Select>
        <InfoTip text="The coding tool this agent's workbench jobs run. Auto uses the profile's first harness. The effort models below are handed to it in its own syntax." />
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Effort</span>
        {EFFORTS.map(([key, label]) => (
          <span key={key} className="flex items-center gap-1">
            <span className="text-xs text-muted">{label}</span>
            <Select
              size="sm"
              value={def.workbenchModels?.[key] ?? ''}
              onChange={(e) => void save({ workbenchModels: { [key]: e.target.value || null } })}
              className="w-44"
            >
              <option value="">org default</option>
              {models.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.id}
                </option>
              ))}
            </Select>
          </span>
        ))}
        <InfoTip text="What low / medium / high effort means for THIS agent. Blank follows the org-wide Workbench model roles on /models. Agents pick the effort; these decide the model." />
      </div>
    </div>
  )
}

/** Explicit per-agent repo grants — the workbench touches ONLY these. */
function WorkbenchRepos({ agentId }: { agentId: string }) {
  const qc = useQueryClient()
  const { data, isLoading } = useQuery({
    queryKey: ['workbench-repos', agentId],
    queryFn: async (): Promise<{ available: string[]; granted: string[] } | null> => {
      const r = await fetch(`/api/workbench/repos/${agentId}`, { credentials: 'same-origin' })
      if (!r.ok) return null
      return (await r.json()) as { available: string[]; granted: string[] }
    },
  })
  const toggle = async (repo: string, on: boolean) => {
    const next = on ? [...(data?.granted ?? []), repo] : (data?.granted ?? []).filter((r) => r !== repo)
    await fetch(`/api/workbench/repos/${agentId}`, {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ repos: next }),
    })
    await qc.invalidateQueries({ queryKey: ['workbench-repos', agentId] })
  }
  if (isLoading) return <div className="mt-2"><SkeletonRows rows={1} /></div>
  if (!data || data.available.length === 0)
    return (
      <p className="mt-1.5 text-xs text-muted">
        No repositories reachable — connect GitHub under <Link to="/admin" className="text-accent hover:underline">Admin → Org</Link> to grant repos.
      </p>
    )
  return (
    <div className="mt-2 space-y-1">
      <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Repos this agent may work</span>
      <div className="flex max-h-32 flex-wrap gap-1.5 overflow-y-auto">
        {/* Repo grants — the one filter-pill primitive (Chip). */}
        {data.available.map((repo) => {
          const on = data.granted.includes(repo)
          return (
            <Chip key={repo} onSelect={() => void toggle(repo, !on)} selected={on} className="px-2.5 py-0.5 normal-case">
              {repo}
            </Chip>
          )
        })}
      </div>
      {data.granted.length === 0 && <p className="text-xs text-muted">Nothing granted yet — the workbench can't touch any repo.</p>}
    </div>
  )
}
