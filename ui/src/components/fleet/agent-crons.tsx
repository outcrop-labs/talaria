import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { CalendarClock, Loader2, Pause, Pencil, Play, Sparkles, Trash2, Zap } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { confirm } from '@/components/ui/confirm'
import { Generating } from '@/components/ui/generating'
import { Input } from '@/components/ui/input'
import { InfoTip } from '@/components/ui/info-tip'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { Modal } from '@/components/ui/modal'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import { relativeTime } from '@/lib/fleet'
import { parseCronDraft, streamMuse } from '@/lib/muse'
import { cn } from '@/lib/cn'

// Native Hermes crons — the jobs live and fire inside each agent's own
// scheduler; Talaria is the control surface. One panel reused by the admin
// agent modal, the member assistant modal, and the fleet-wide schedules view.
//
// Nobody should have to WRITE cron syntax: the schedule builder covers the
// real shapes (interval / daily / weekdays / weekly / monthly) and compiles
// to the cron expression underneath; "custom" exposes the raw string for the
// rest. Schedules DISPLAY in plain English everywhere.

export interface CronJob {
  id: string
  name: string
  prompt: string
  schedule: string
  enabled: boolean
  state: string
  nextRunAt: string | null
  lastRunAt: string | null
  lastStatus: string | null
  lastError: string | null
}

// ── Schedule model: friendly shapes ⇄ cron/shorthand strings ────────────────
type SchedMode = 'interval' | 'daily' | 'weekdays' | 'weekly' | 'monthly' | 'custom'
interface Sched {
  mode: SchedMode
  every: number
  unit: 'm' | 'h'
  time: string // "HH:MM", the agent's clock (UTC)
  dow: number // 0-6, Sunday=0
  dom: number // 1-31
  custom: string
}
const DEFAULT_SCHED: Sched = { mode: 'daily', every: 30, unit: 'm', time: '09:00', dow: 1, dom: 1, custom: '' }
const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday']

function timeParts(t: string): { h: number; m: number } {
  const [h = 9, m = 0] = t.split(':').map((x) => Number(x))
  return { h: Math.min(23, Math.max(0, h || 0)), m: Math.min(59, Math.max(0, m || 0)) }
}

/** Compile the builder state to what Hermes stores. */
function schedToString(s: Sched): string {
  const { h, m } = timeParts(s.time)
  switch (s.mode) {
    case 'interval':
      return `every ${Math.max(1, s.every)}${s.unit}`
    case 'daily':
      return `${m} ${h} * * *`
    case 'weekdays':
      return `${m} ${h} * * 1-5`
    case 'weekly':
      return `${m} ${h} * * ${s.dow}`
    case 'monthly':
      return `${m} ${h} ${Math.min(31, Math.max(1, s.dom))} * *`
    case 'custom':
      return s.custom.trim()
  }
}

/** Best-effort inverse — recognized shapes open in the builder, the rest as custom. */
function parseSchedule(raw: string): Sched {
  const s = raw.trim()
  const interval = /^(?:every\s+)?(\d+)\s*(m|min|minutes?|h|hours?)$/i.exec(s)
  if (interval) {
    return { ...DEFAULT_SCHED, mode: 'interval', every: Number(interval[1]), unit: interval[2]!.startsWith('h') ? 'h' : 'm' }
  }
  const cron = /^(\d{1,2})\s+(\d{1,2})\s+(\S+)\s+(\S+)\s+(\S+)$/.exec(s)
  if (cron) {
    const [, m, h, dom, mon, dow] = cron
    const time = `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`
    if (dom === '*' && mon === '*' && dow === '*') return { ...DEFAULT_SCHED, mode: 'daily', time }
    if (dom === '*' && mon === '*' && dow === '1-5') return { ...DEFAULT_SCHED, mode: 'weekdays', time }
    if (dom === '*' && mon === '*' && /^\d$/.test(dow!)) return { ...DEFAULT_SCHED, mode: 'weekly', time, dow: Number(dow) }
    if (/^\d{1,2}$/.test(dom!) && mon === '*' && dow === '*') return { ...DEFAULT_SCHED, mode: 'monthly', time, dom: Number(dom) }
  }
  return { ...DEFAULT_SCHED, mode: 'custom', custom: s }
}

/** Plain-English schedule for display — the cron string stays in the tooltip. */
export function describeSchedule(raw: string): string {
  const s = parseSchedule(raw)
  const t = s.time
  switch (s.mode) {
    case 'interval':
      return s.unit === 'h' ? (s.every === 1 ? 'Every hour' : `Every ${s.every} hours`) : `Every ${s.every} minutes`
    case 'daily':
      return `Every day at ${t}`
    case 'weekdays':
      return `Weekdays at ${t}`
    case 'weekly':
      return `Every ${DAYS[s.dow]} at ${t}`
    case 'monthly':
      return `Monthly on day ${s.dom} at ${t}`
    case 'custom':
      return raw
  }
}

/** The scheduling UI: pick a shape, fill the blanks, see it in English. */
function ScheduleBuilder({ value, onChange }: { value: Sched; onChange: (s: Sched) => void }) {
  const set = (patch: Partial<Sched>) => onChange({ ...value, ...patch })
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        <Select size="sm" value={value.mode} onChange={(e) => set({ mode: e.target.value as SchedMode })} className="w-36">
          <option value="interval">Repeating</option>
          <option value="daily">Every day</option>
          <option value="weekdays">Weekdays</option>
          <option value="weekly">Weekly</option>
          <option value="monthly">Monthly</option>
          <option value="custom">Custom (cron)</option>
        </Select>
        {value.mode === 'interval' && (
          <>
            <span className="text-xs text-muted">every</span>
            <Input size="sm" type="number" min={1} max={999} value={value.every} onChange={(e) => set({ every: Math.max(1, Number(e.target.value) || 1) })} className="w-20" />
            <Select size="sm" value={value.unit} onChange={(e) => set({ unit: e.target.value as 'm' | 'h' })} className="w-28">
              <option value="m">minutes</option>
              <option value="h">hours</option>
            </Select>
          </>
        )}
        {value.mode === 'weekly' && (
          <Select size="sm" value={String(value.dow)} onChange={(e) => set({ dow: Number(e.target.value) })} className="w-32">
            {DAYS.map((d, i) => (
              <option key={d} value={i}>
                {d}
              </option>
            ))}
          </Select>
        )}
        {value.mode === 'monthly' && (
          <>
            <span className="text-xs text-muted">on day</span>
            <Input size="sm" type="number" min={1} max={31} value={value.dom} onChange={(e) => set({ dom: Math.min(31, Math.max(1, Number(e.target.value) || 1)) })} className="w-16" />
          </>
        )}
        {['daily', 'weekdays', 'weekly', 'monthly'].includes(value.mode) && (
          <>
            <span className="text-xs text-muted">at</span>
            <Input size="sm" type="time" value={value.time} onChange={(e) => set({ time: e.target.value || '09:00' })} className="w-28" />
          </>
        )}
        {value.mode === 'custom' && (
          <Input size="sm" value={value.custom} onChange={(e) => set({ custom: e.target.value })} placeholder="0 9 * * 1-5" className="w-44 font-[var(--font-mono)]" />
        )}
        <InfoTip text="Times are the agent's clock (UTC). Underneath this compiles to standard cron syntax — Custom accepts any 5-field expression or an interval like 'every 2h'." />
      </div>
      <div className="text-xs text-muted">
        → <span className="text-fg">{describeSchedule(schedToString(value)) || '…'}</span>
        {value.mode !== 'custom' && <span className="ml-2 font-[var(--font-mono)] text-[10px]">{schedToString(value)}</span>}
      </div>
    </div>
  )
}

/** Next-run label — absolute short form ("Wed 9:00 AM"); relativeTime is past-oriented. */
const fmtNext = (iso: string) =>
  new Date(iso).toLocaleString(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' })

function jobDot(j: CronJob): string {
  if (!j.enabled || j.state === 'paused') return 'var(--theme-line)'
  if (j.lastStatus && /error|fail/i.test(j.lastStatus)) return 'var(--theme-danger)'
  return 'var(--theme-success)'
}

function CronRow({
  job,
  onAction,
  onEdit,
  busy,
  agentLabel,
}: {
  job: CronJob
  onAction?: (action: 'pause' | 'resume' | 'run' | 'remove') => void
  /** Present ⇒ the row is editable; called with the patch on save. */
  onEdit?: (patch: { name: string; schedule: string; prompt: string }) => Promise<boolean>
  busy?: boolean
  agentLabel?: string
}) {
  const [expanded, setExpanded] = useState(false)
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(job.name)
  const [sched, setSched] = useState<Sched>(() => parseSchedule(job.schedule))
  const [prompt, setPrompt] = useState(job.prompt)
  const paused = !job.enabled || job.state === 'paused'

  const startEdit = () => {
    setName(job.name)
    setSched(parseSchedule(job.schedule))
    setPrompt(job.prompt)
    setEditing(true)
    setExpanded(true)
  }
  const saveEdit = async () => {
    if (!onEdit) return
    const ok = await onEdit({ name: name.trim(), schedule: schedToString(sched), prompt: prompt.trim() })
    if (ok) setEditing(false)
  }

  return (
    <li className="px-3.5 py-3">
      <div className="flex items-center gap-2.5">
        <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: jobDot(job) }} title={paused ? 'paused' : job.state} />
        <button type="button" onClick={() => setExpanded((v) => !v)} className="min-w-0 flex-1 text-left">
          <span className="font-sans text-sm font-medium text-fg">{job.name}</span>
          {agentLabel && <span className="ml-2 text-xs text-muted">{agentLabel}</span>}
          <span className="ml-2 font-sans text-xs text-accent" title={job.schedule}>
            {describeSchedule(job.schedule)}
          </span>
        </button>
        <span className="shrink-0 text-xs text-muted">
          {paused ? 'paused' : job.nextRunAt ? `next ${fmtNext(job.nextRunAt)}` : ''}
        </span>
        {onAction && (
          <span className={cn('flex shrink-0 items-center', busy && 'pointer-events-none opacity-40')}>
            {onEdit && (
              <button
                type="button"
                title="Edit"
                onClick={startEdit}
                className="grid h-7 w-7 place-items-center rounded-lg text-muted transition-colors hover:bg-card hover:text-fg"
              >
                <Pencil size={13} />
              </button>
            )}
            <button
              type="button"
              title="Run on the next tick (≤60s)"
              onClick={() => onAction('run')}
              className="grid h-7 w-7 place-items-center rounded-lg text-muted transition-colors hover:bg-card hover:text-fg"
            >
              <Zap size={14} />
            </button>
            <button
              type="button"
              title={paused ? 'Resume' : 'Pause'}
              onClick={() => onAction(paused ? 'resume' : 'pause')}
              className="grid h-7 w-7 place-items-center rounded-lg text-muted transition-colors hover:bg-card hover:text-fg"
            >
              {paused ? <Play size={14} fill="currentColor" /> : <Pause size={14} fill="currentColor" />}
            </button>
            <button
              type="button"
              title="Delete"
              onClick={() => onAction('remove')}
              className="grid h-7 w-7 place-items-center rounded-lg text-muted transition-colors hover:bg-card hover:text-[color:var(--theme-danger)]"
            >
              <Trash2 size={14} />
            </button>
          </span>
        )}
      </div>
      {expanded && !editing && (
        <div className="mt-2.5 space-y-1.5 pl-4">
          <div className="whitespace-pre-wrap rounded-lg border border-line-subtle p-3 font-sans text-xs leading-5 text-muted">{job.prompt}</div>
          <div className="text-[11px] text-muted">
            {job.lastRunAt ? `last ran ${relativeTime(job.lastRunAt)}${job.lastStatus ? ` · ${job.lastStatus}` : ''}` : 'never ran'}
            {job.lastError && <span className="text-[color:var(--theme-danger)]"> · {job.lastError}</span>}
          </div>
        </div>
      )}
      {editing && (
        <div className="mt-2.5 space-y-3 rounded-lg border border-line-subtle p-3.5 pl-4">
          <div className="flex items-center gap-2">
            <Input size="sm" value={name} onChange={(e) => setName(e.target.value)} maxLength={80} className="w-56" />
          </div>
          <ScheduleBuilder value={sched} onChange={setSched} />
          <Textarea autoGrow rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)} className="max-h-64" maxLength={20_000} />
          <div className="flex items-center gap-2">
            <Button size="sm" disabled={busy || !name.trim() || !prompt.trim() || !schedToString(sched)} onClick={() => void saveEdit()}>
              Save changes
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setEditing(false)}>
              Cancel
            </Button>
          </div>
        </div>
      )}
    </li>
  )
}

/** The shape of the jobs list while it loads: dot + name + schedule + actions. */
function CronListSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <ul aria-hidden className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
      {Array.from({ length: rows }, (_, i) => (
        <li key={i} className="flex items-center gap-2.5 px-3.5 py-3">
          <Skeleton className="h-2 w-2 shrink-0 rounded-full" delay={i * 0.12} />
          <Skeleton className="h-3 w-36 rounded-full" delay={i * 0.12} />
          <Skeleton className="h-2.5 w-28 rounded-full" delay={i * 0.12 + 0.12} />
          <span className="ml-auto flex shrink-0 items-center gap-1">
            <Skeleton className="h-7 w-7 rounded-lg" delay={i * 0.12} />
            <Skeleton className="h-7 w-7 rounded-lg" delay={i * 0.12 + 0.06} />
            <Skeleton className="h-7 w-7 rounded-lg" delay={i * 0.12 + 0.12} />
          </span>
        </li>
      ))}
    </ul>
  )
}

function CronForm({
  onCreate,
  busy,
  disabled = false,
  children,
}: {
  onCreate: (input: { name: string; schedule: string; prompt: string }) => Promise<boolean>
  busy: boolean
  /** Hold the create button (e.g. while the fleet agent list is still loading,
   *  when the target set would silently be empty). */
  disabled?: boolean
  /** Extra fields (e.g. the fleet agent picker) rendered above the buttons. */
  children?: React.ReactNode
}) {
  const [name, setName] = useState('')
  const [sched, setSched] = useState<Sched>(DEFAULT_SCHED)
  const [prompt, setPrompt] = useState('')
  const [draftAsk, setDraftAsk] = useState('')
  const [drafting, setDrafting] = useState(false)
  const [draftErr, setDraftErr] = useState<string | null>(null)
  const ok = name.trim() && schedToString(sched) && prompt.trim()

  // Natural language → {name, schedule, prompt} via the drafting muse.
  const draft = async () => {
    const ask = draftAsk.trim()
    if (!ask) return
    setDrafting(true)
    setDraftErr(null)
    try {
      const full = await streamMuse({ kind: 'cron', instruction: ask }, () => {})
      const j = parseCronDraft(full)
      if (!j) return setDraftErr('could not turn that into a job. Try rephrasing')
      setName(j.name)
      setSched(parseSchedule(j.schedule))
      setPrompt(j.prompt)
      setDraftAsk('')
    } catch (e) {
      setDraftErr((e as Error).message)
    } finally {
      setDrafting(false)
    }
  }

  return (
    <div className="space-y-5 rounded-xl border border-line-subtle p-5">
      <div className="flex items-end gap-2.5">
        <Sparkles size={14} className="mb-3 shrink-0 text-accent" />
        <Textarea
          autoGrow
          rows={1}
          value={draftAsk}
          onChange={(e) => setDraftAsk(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              if (!drafting && draftAsk.trim()) void draft()
            }
          }}
          placeholder="Describe it, e.g. “every weekday morning, summarize my inbox into a brief”"
          className="max-h-32 text-sm"
        />
        <Button variant="outline" className="shrink-0 whitespace-nowrap" onClick={() => void draft()} disabled={drafting || !draftAsk.trim()}>
          {drafting ? 'Drafting' : 'Draft'}
        </Button>
      </div>
      {draftErr && <p className="text-xs text-[color:var(--theme-danger)]">{draftErr}</p>}
      {drafting && <Generating label="Designing the job: name, schedule, and the prompt it runs" lines={2} />}

      <div className="grid gap-4 sm:grid-cols-2">
        <div>
          <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Name</label>
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="weekly-recap" maxLength={80} />
        </div>
      </div>
      <div>
        <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">When</label>
        <ScheduleBuilder value={sched} onChange={setSched} />
      </div>
      <div>
        <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">What it does</label>
        <Textarea
          autoGrow
          rows={3}
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          placeholder="What should it do each time? Written as a self-contained instruction."
          className="max-h-64"
          maxLength={20_000}
        />
      </div>
      {children}
      <div className="flex items-center gap-3">
        <Button
          className="shrink-0 whitespace-nowrap"
          disabled={!ok || busy || disabled}
          onClick={() =>
            void onCreate({ name: name.trim(), schedule: schedToString(sched), prompt: prompt.trim() }).then((created) => {
              if (created) {
                setName('')
                setSched(DEFAULT_SCHED)
                setPrompt('')
              }
            })
          }
        >
          {busy && <Loader2 size={13} className="animate-spin" />}
          {busy ? 'Creating' : 'Create job'}
        </Button>
      </div>
    </div>
  )
}

/** One agent's cron jobs: list + create + edit/pause/run/delete. */
export function CronsPanel({ agentId }: { agentId: string }) {
  const qc = useQueryClient()
  const key = ['agent-crons', agentId]
  const { data, isLoading, error } = useQuery({
    queryKey: key,
    queryFn: async (): Promise<CronJob[]> => {
      const r = await fetch(`/api/fleet/agents/${agentId}/crons`, { credentials: 'same-origin' })
      const j = (await r.json().catch(() => null)) as { jobs?: CronJob[]; error?: string } | null
      if (!r.ok || !j?.jobs) throw new Error(j?.error ?? 'could not load schedules')
      return j.jobs
    },
  })
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: key })

  const create = async (input: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    setBusy(true)
    setErr(null)
    try {
      const r = await fetch(`/api/fleet/agents/${agentId}/crons`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(input),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) {
        setErr(j.error ?? 'could not create the job')
        return false
      }
      await refresh()
      return true
    } finally {
      setBusy(false)
    }
  }

  const edit = async (jobId: string, patch: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    setBusy(true)
    setErr(null)
    try {
      const r = await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(patch),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) {
        setErr(j.error ?? 'could not save the job')
        return false
      }
      await refresh()
      return true
    } finally {
      setBusy(false)
    }
  }

  const act = async (jobId: string, action: 'pause' | 'resume' | 'run' | 'remove') => {
    if (action === 'remove' && !(await confirm({ title: 'Delete scheduled job', message: 'Delete this scheduled job?', confirmLabel: 'Delete', danger: true }))) return
    setBusy(true)
    setErr(null)
    try {
      const r =
        action === 'remove'
          ? await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, { method: 'DELETE', credentials: 'same-origin' })
          : await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, {
              method: 'POST',
              credentials: 'same-origin',
              headers: { 'content-type': 'application/json' },
              body: JSON.stringify({ action }),
            })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) setErr(j.error ?? `could not ${action}`)
      await refresh()
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-1.5">
        <span className="text-xs uppercase tracking-wide text-muted">Schedules</span>
        <InfoTip text="Recurring jobs the agent runs on its own native scheduler — they keep firing even when Talaria is down." />
      </div>
      {isLoading ? (
        <CronListSkeleton />
      ) : error ? (
        <EmptyState icon="◌" title="Schedules unavailable" hint={(error as Error).message} />
      ) : (data ?? []).length === 0 ? (
        <EmptyState icon={<CalendarClock size={22} />} title="Nothing scheduled" hint="Give it a recurring job below." />
      ) : (
        <ul className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
          {data!.map((j) => (
            <CronRow key={j.id} job={j} busy={busy} onAction={(a) => void act(j.id, a)} onEdit={(patch) => edit(j.id, patch)} />
          ))}
        </ul>
      )}
      <CronForm onCreate={create} busy={busy} />
      {err && <p className="text-xs text-[color:var(--theme-danger)]">{err}</p>}
    </div>
  )
}

interface FleetCronAgent {
  id: string
  slug: string
  displayName: string
  jobs: CronJob[]
  error?: string
}

/** Fleet-wide schedules (admin): every agent's jobs + create-across-agents. */
export function FleetCronsModal({ onClose }: { onClose: () => void }) {
  const qc = useQueryClient()
  const { data, isLoading } = useQuery({
    queryKey: ['fleet-crons'],
    queryFn: async (): Promise<FleetCronAgent[]> => {
      const r = await fetch('/api/fleet/crons', { credentials: 'same-origin' })
      if (!r.ok) throw new Error('could not load fleet schedules')
      return ((await r.json()) as { agents: FleetCronAgent[] }).agents
    },
  })
  const agents = data ?? []
  const [selected, setSelected] = useState<Set<string> | null>(null) // null = all
  const [busy, setBusy] = useState(false)
  const [summary, setSummary] = useState<string | null>(null)
  const chosen = selected ?? new Set(agents.map((a) => a.id))

  const toggle = (id: string) => {
    const next = new Set(chosen)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    setSelected(next)
  }

  const create = async (input: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    setBusy(true)
    setSummary(null)
    try {
      const r = await fetch('/api/fleet/crons', {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ ...input, agentIds: [...chosen] }),
      })
      const j = (await r.json().catch(() => null)) as { results?: Array<{ ok: boolean; error?: string }>; error?: string } | null
      if (!j?.results) {
        setSummary(j?.error ?? 'could not create jobs')
        return false
      }
      const failed = j.results.filter((x) => !x.ok)
      setSummary(failed.length === 0 ? `Created on ${j.results.length} agents (staggered).` : `Created on ${j.results.length - failed.length}, failed on ${failed.length}: ${failed[0]?.error ?? ''}`)
      await qc.invalidateQueries({ queryKey: ['fleet-crons'] })
      return failed.length === 0
    } finally {
      setBusy(false)
    }
  }

  const act = async (agentId: string, jobId: string, action: 'pause' | 'resume' | 'run' | 'remove') => {
    if (action === 'remove' && !(await confirm({ title: 'Delete scheduled job', message: 'Delete this scheduled job?', confirmLabel: 'Delete', danger: true }))) return
    if (action === 'remove') await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, { method: 'DELETE', credentials: 'same-origin' })
    else
      await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ action }),
      })
    await qc.invalidateQueries({ queryKey: ['fleet-crons'] })
  }

  const edit = async (agentId: string, jobId: string, patch: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    const r = await fetch(`/api/fleet/agents/${agentId}/crons/${jobId}`, {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(patch),
    })
    await qc.invalidateQueries({ queryKey: ['fleet-crons'] })
    return r.ok
  }

  const withJobs = agents.flatMap((a) => a.jobs.map((j) => ({ agent: a, job: j })))

  return (
    <Modal
      open
      onClose={onClose}
      takeover
      title={
        <span className="flex items-center gap-1.5">
          Schedules
          <InfoTip text="Jobs run inside each agent's own scheduler and keep firing even if Talaria is down. Fixed-time jobs created here are staggered 2 minutes per agent so the fleet doesn't hit the models at once." />
        </span>
      }
    >
      <div className="space-y-5">
        {isLoading ? (
          <CronListSkeleton />
        ) : withJobs.length === 0 ? (
          <EmptyState icon={<CalendarClock size={22} />} title="Nothing scheduled anywhere" hint="Create the first job below." />
        ) : (
          <ul className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
            {withJobs.map(({ agent, job }) => (
              <CronRow
                key={`${agent.id}-${job.id}`}
                job={job}
                agentLabel={agent.displayName}
                onAction={(a) => void act(agent.id, job.id, a)}
                onEdit={(patch) => edit(agent.id, job.id, patch)}
              />
            ))}
          </ul>
        )}
        {agents.some((a) => a.error) && (
          <p className="text-xs text-[color:var(--theme-warning)]">
            Unreachable: {agents.filter((a) => a.error).map((a) => a.displayName).join(', ')}. Are they running?
          </p>
        )}

        <div>
          <div className="mb-1.5 text-[11px] uppercase tracking-wide text-muted">New job across agents</div>
          {/* While agents load, `chosen` would be an empty set — a job created
              now would target NOBODY. Hold the create button and show the
              chip row's shape until the roster resolves. */}
          <CronForm onCreate={create} busy={busy} disabled={isLoading}>
            <div className="flex flex-wrap gap-1.5">
              {isLoading &&
                Array.from({ length: 4 }, (_, i) => <Skeleton key={i} className="h-6 w-20 rounded-full" delay={i * 0.12} />)}
              {agents.map((a) => (
                <button
                  key={a.id}
                  type="button"
                  onClick={() => toggle(a.id)}
                  className={cn(
                    'rounded-full border px-2.5 py-0.5 text-[11px] transition-colors',
                    chosen.has(a.id) ? 'border-[var(--theme-accent)] text-accent' : 'border-line-subtle text-muted hover:border-line',
                  )}
                >
                  {a.displayName}
                </button>
              ))}
            </div>
          </CronForm>
          {summary && <p className="mt-2 text-xs text-muted">{summary}</p>}
        </div>
      </div>
    </Modal>
  )
}
