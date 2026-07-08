import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { stringify as stringifyYaml } from 'yaml'
import { Loader2, Check, Lock, X, RotateCcw, Plug } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Markdown } from '@/components/ui/markdown'
import { EmptyState } from '@/components/ui/empty-state'
import { cn } from '@/lib/cn'
import { relativeTime, useFleet } from '@/lib/fleet'
import { AgentConfigForm } from '@/components/fleet/agent-editor'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { patchAgentMeta, type AgentDef, type LlmEndpoint, type ModelTarget } from '@/lib/fleet-defs'

type Tab = 'summary' | 'config' | 'skills' | 'memory' | 'mcp' | 'versions'
const TABS: { id: Tab; label: string }[] = [
  { id: 'summary', label: 'Summary' },
  { id: 'config', label: 'Config' },
  { id: 'skills', label: 'Skills' },
  { id: 'memory', label: 'Memory' },
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
    <Modal open={open} onClose={onClose} title={`${def.displayName} · v${def.currentVersion}`} width="max-w-3xl">
      <div className="flex gap-1 border-b border-line-subtle">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            onClick={() => setTab(t.id)}
            className={cn(
              'relative px-3 py-2 text-sm transition-colors',
              tab === t.id ? 'text-fg' : 'text-muted hover:text-fg',
            )}
          >
            {t.label}
            {tab === t.id && <span className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-accent" />}
          </button>
        ))}
      </div>

      <div className="max-h-[65vh] overflow-y-auto pt-4">
        {tab === 'summary' && <SummaryTab def={def} isAdmin={isAdmin} />}
        {tab === 'config' &&
          (isAdmin ? (
            <AgentConfigForm def={def} endpoints={endpoints} onSaved={onClose} />
          ) : (
            <ReadOnlyConfig def={def} />
          ))}
        {tab === 'skills' && <SkillsTab slug={def.slug} isAdmin={isAdmin} />}
        {tab === 'memory' && <MemoryTab def={def} isAdmin={isAdmin} />}
        {tab === 'mcp' && <McpTab def={def} isAdmin={isAdmin} />}
        {tab === 'versions' && <VersionsTab def={def} isAdmin={isAdmin} />}
      </div>
    </Modal>
  )
}

// ── Summary ───────────────────────────────────────────────────────────────────
function TargetChip({ t, name }: { t: ModelTarget; name?: string }) {
  const local = /inference|vllm|ollama|spark|local/.test(t.endpoint)
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-line px-2 py-0.5 text-xs">
      {name && <span className="font-semibold text-fg">{name}</span>}
      <span className="text-muted">{t.model}</span>
      <span className={local ? 'text-[color:var(--theme-success)]' : 'text-accent'}>{local ? 'self-hosted' : 'cloud'}</span>
    </span>
  )
}

function SummaryTab({ def, isAdmin }: { def: AgentDef; isAdmin: boolean }) {
  const qc = useQueryClient()
  const cfg = def.latest?.config
  const { data: fleet } = useFleet()
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
        <div className="mb-1 text-xs uppercase tracking-wide text-muted">Role</div>
        {isAdmin ? (
          <Input size="sm" value={role} onChange={(e) => setRole(e.target.value)} onBlur={() => void saveRole()} placeholder="e.g. Support Lead" className="max-w-xs" />
        ) : (
          <div className="text-fg">{def.role ?? '—'}</div>
        )}
      </div>
      <div className="grid grid-cols-2 gap-3">
        <Stat label="Model id" value={def.model} />
        <Stat label="Department" value={def.department} />
        <Stat label="Management" value={def.managed ? 'Talaria-managed' : 'legacy stack'} />
        <Stat label="Version" value={`v${def.currentVersion}`} />
        {stat && <Stat label="Conversations" value={String(stat.conversations)} />}
        {stat && <Stat label="Messages" value={String(stat.messages)} />}
        {stat?.lastUsed && <Stat label="Last used" value={relativeTime(stat.lastUsed)} />}
      </div>
      <div>
        <div className="mb-1.5 text-xs uppercase tracking-wide text-muted">Models</div>
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
    </div>
  )
}
const Stat = ({ label, value }: { label: string; value: string }) => (
  <div>
    <div className="text-xs uppercase tracking-wide text-muted">{label}</div>
    <div className="truncate text-fg" title={value}>{value}</div>
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
          <div className="mb-1 text-xs uppercase tracking-wide text-muted">Soul</div>
          <div className="max-h-64 overflow-y-auto rounded-lg border border-line-subtle p-3 text-xs">
            <Markdown>{def.latest.soul}</Markdown>
          </div>
        </div>
      )}
    </div>
  )
}
const Field = ({ label, value }: { label: string; value: string }) => (
  <div className="flex gap-3">
    <span className="w-24 shrink-0 text-xs uppercase tracking-wide text-muted">{label}</span>
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
  const { data } = useQuery({
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
      body: JSON.stringify({ content: `# ${name}\n\ndescription: what this skill is for\n\n## Steps\n\n1. …\n` }),
    })
    setNewName('')
    await qc.invalidateQueries({ queryKey: ['skills'] })
    setOpen(name)
  }

  return (
    <div className="space-y-3">
      {skills.length === 0 ? (
        <EmptyState icon="✦" title="No skills yet" hint={isAdmin ? 'Add one below.' : undefined} />
      ) : (
        <div className="divide-y divide-line-subtle">
          {skills.map((s) => (
            <button key={s.name} type="button" onClick={() => setOpen(s.name)} className="flex w-full items-baseline gap-3 py-2.5 text-left">
              <span className="shrink-0 text-sm font-medium text-fg">{s.name}</span>
              <span className="min-w-0 truncate text-sm text-muted">{s.description}</span>
              {s.files.length > 1 && <span className="ml-auto shrink-0 text-xs text-muted">{s.files.length} files</span>}
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
    if (!confirm(`Delete the "${name}" skill?`)) return
    await fetch(`/api/skills/${slug}/${name}`, { method: 'DELETE' })
    await qc.invalidateQueries({ queryKey: ['skills'] })
    onClose()
  }
  return (
    <InternalEditorModal
      open
      onClose={onClose}
      title={`${name} · SKILL.md`}
      subtitle="Read live — the agent picks up edits on its next run."
      value={data?.content ?? ''}
      editable={isAdmin}
      saving={busy}
      onSave={save}
      history={{ kind: 'skill', owner: slug, name }}
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
  if (!def.managed) return <EmptyState icon="❖" title="Not managed" hint="Memory reads through the managed container — migrate this agent first." />
  if (isLoading) return <div className="text-sm text-muted">Reading memory…</div>
  if (error) return <EmptyState icon="❖" title="Can't reach the agent" hint={(error as Error).message} />
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-3">
        {data?.container && <span className="min-w-0 truncate text-xs text-muted">{data.container} · the agent edits this too — last writer wins</span>}
        {isAdmin && (
          <Button size="sm" className="ml-auto shrink-0" onClick={() => setEditing(true)}>
            Edit
          </Button>
        )}
      </div>
      {data?.content ? (
        <div className="max-h-[52vh] overflow-y-auto text-sm">
          <Markdown>{data.content}</Markdown>
        </div>
      ) : (
        <EmptyState icon="❖" title="No memory yet" hint="The agent hasn't written anything down." />
      )}
      {editing && (
        <InternalEditorModal
          open
          onClose={() => setEditing(false)}
          title={`${def.displayName} · MEMORY.md`}
          subtitle="The agent maintains this itself; your edits are snapshotted and revertible."
          value={data?.content ?? ''}
          editable={isAdmin}
          saving={busy}
          onSave={save}
          history={{ kind: 'memory', id: def.id }}
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
  const { data } = useQuery({
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
      {servers.length === 0 ? (
        <div className="text-sm text-muted">No MCP servers connected.</div>
      ) : (
        <div className="divide-y divide-line-subtle">
          {servers.map((s) => {
            const probe = probes[s.name]
            return (
              <div key={s.name} className="flex items-center gap-3 py-2.5 text-sm">
                <span className="w-28 shrink-0 truncate font-medium text-fg">{s.name}</span>
                <span className="min-w-0 flex-1 truncate text-muted">{s.url}</span>
                {probe === 'testing' ? (
                  <Loader2 size={13} className="shrink-0 animate-spin text-muted" />
                ) : probe ? (
                  <span className="flex shrink-0 items-center gap-1 text-xs" style={{ color: PROBE_UI[probe.state].color }} title={probe.detail}>
                    {PROBE_UI[probe.state].icon} {PROBE_UI[probe.state].label}
                  </span>
                ) : null}
                <button type="button" onClick={() => void test(s)} className="shrink-0 text-xs text-muted hover:text-accent">
                  Test
                </button>
                {isAdmin && (
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => confirm(`Remove "${s.name}"?`) && void edit({ remove: [s.name] })}
                    className="shrink-0 text-xs text-muted hover:text-[color:var(--theme-danger)]"
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
          <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} placeholder="name" className="w-28" />
          <Input size="sm" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://host:port/mcp" className="flex-1" />
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
  const { data } = useQuery({
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
    if (!confirm(`Revert ${def.displayName} to v${v}? This publishes it as a new version.`)) return
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
  return versions.length === 0 ? (
    <div className="text-sm text-muted">No version history.</div>
  ) : (
    <div className="divide-y divide-line-subtle">
      <div className="flex items-center gap-2 pb-2.5">
        <span className="text-xs text-muted">Reverting publishes the old content as a new version.</span>
        <Button variant="outline" size="sm" className="ml-auto" onClick={() => setConfigOpen(true)}>
          Config history
        </Button>
      </div>
      {configOpen && (
        <InternalEditorModal
          open
          onClose={() => setConfigOpen(false)}
          title={`${def.displayName} · config`}
          subtitle="The rendered model/tool config per version — click a revision to see what changed."
          value={stringifyYaml(def.latest?.config ?? {})}
          editable={false}
          onSave={() => {}}
          history={{ kind: 'config', id: def.id }}
          mode="plain"
        />
      )}
      {versions.map((v) => (
        <div key={v.version} className="flex items-center gap-3 py-2.5 text-sm">
          <span className={cn('w-12 shrink-0 font-[var(--font-mono)]', v.version === def.currentVersion ? 'text-accent' : 'text-muted')}>v{v.version}</span>
          <span className="min-w-0 flex-1 truncate text-fg">{v.note ?? '—'}</span>
          <span className="shrink-0 text-xs text-muted">{v.createdBy ?? 'system'} · {relativeTime(v.createdAt)}</span>
          {isAdmin && v.version !== def.currentVersion && (
            <button type="button" disabled={busy !== null} onClick={() => void revert(v.version)} className="flex shrink-0 items-center gap-1 text-xs text-muted hover:text-accent">
              <RotateCcw size={12} /> {busy === v.version ? 'reverting…' : 'revert'}
            </button>
          )}
          {v.version === def.currentVersion && <span className="shrink-0 text-xs" style={{ color: 'var(--theme-success)' }}>current</span>}
        </div>
      ))}
    </div>
  )
}
