import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { CalendarClock, Loader2, Pause, Play, Sparkles, Trash2, Zap } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { confirm } from '@/components/ui/confirm'
import { Generating } from '@/components/ui/generating'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Modal } from '@/components/ui/modal'
import { EmptyState } from '@/components/ui/empty-state'
import { relativeTime } from '@/lib/fleet'
import { parseCronDraft, streamMuse } from '@/lib/muse'
import { cn } from '@/lib/cn'

// Native Hermes crons — the jobs live and fire inside each agent's own
// scheduler; Talaria is the control surface. One panel reused by the admin
// agent modal, the member assistant modal, and the fleet-wide schedules view.

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

// Hermes-native shorthand ('every 2h') and cron exprs both work.
const PRESETS = [
  { label: 'Every 30 min', value: 'every 30m' },
  { label: 'Hourly', value: 'every 1h' },
  { label: 'Daily 9:00', value: '0 9 * * *' },
  { label: 'Weekdays 9:00', value: '0 9 * * 1-5' },
  { label: 'Mondays 9:00', value: '0 9 * * 1' },
] as const

function scheduleHint(): string {
  return 'A cron expression ("0 9 * * 1-5") or plain interval ("every 2h", "30m"). Times are the agent\'s clock (UTC).'
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
  busy,
  agentLabel,
}: {
  job: CronJob
  onAction?: (action: 'pause' | 'resume' | 'run' | 'remove') => void
  busy?: boolean
  agentLabel?: string
}) {
  const [expanded, setExpanded] = useState(false)
  const paused = !job.enabled || job.state === 'paused'
  return (
    <li className="px-3.5 py-3">
      <div className="flex items-center gap-2.5">
        <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: jobDot(job) }} title={paused ? 'paused' : job.state} />
        <button type="button" onClick={() => setExpanded((v) => !v)} className="min-w-0 flex-1 text-left">
          <span className="text-sm font-medium text-fg">{job.name}</span>
          {agentLabel && <span className="ml-2 text-xs text-muted">{agentLabel}</span>}
          <span className="ml-2 font-[var(--font-mono)] text-xs text-accent">{job.schedule}</span>
        </button>
        <span className="shrink-0 text-xs text-muted">
          {paused ? 'paused' : job.nextRunAt ? `next ${fmtNext(job.nextRunAt)}` : ''}
        </span>
        {onAction && (
          <span className={cn('flex shrink-0 items-center', busy && 'pointer-events-none opacity-40')}>
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
      {expanded && (
        <div className="mt-2.5 space-y-1.5 pl-4">
          <pre className="whitespace-pre-wrap rounded-lg border border-line-subtle p-3 font-[var(--font-mono)] text-xs leading-5 text-muted">{job.prompt}</pre>
          <div className="text-[11px] text-muted">
            {job.lastRunAt ? `last ran ${relativeTime(job.lastRunAt)}${job.lastStatus ? ` · ${job.lastStatus}` : ''}` : 'never ran'}
            {job.lastError && <span className="text-[color:var(--theme-danger)]"> · {job.lastError}</span>}
          </div>
        </div>
      )}
    </li>
  )
}

function CronForm({
  onCreate,
  busy,
  children,
}: {
  onCreate: (input: { name: string; schedule: string; prompt: string }) => Promise<boolean>
  busy: boolean
  /** Extra fields (e.g. the fleet agent picker) rendered above the buttons. */
  children?: React.ReactNode
}) {
  const [name, setName] = useState('')
  const [schedule, setSchedule] = useState('')
  const [prompt, setPrompt] = useState('')
  const [draftAsk, setDraftAsk] = useState('')
  const [drafting, setDrafting] = useState(false)
  const [draftErr, setDraftErr] = useState<string | null>(null)
  const ok = name.trim() && schedule.trim() && prompt.trim()

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
      setSchedule(j.schedule)
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
        <div>
          <label className="mb-1.5 block text-[11px] uppercase tracking-wide text-muted">Schedule</label>
          <Input value={schedule} onChange={(e) => setSchedule(e.target.value)} placeholder="0 9 * * 1-5 · every 2h" maxLength={120} />
        </div>
      </div>
      <div className="flex flex-wrap gap-2">
        {PRESETS.map((p) => (
          <button
            key={p.value}
            type="button"
            onClick={() => setSchedule(p.value)}
            className={cn(
              'rounded-full border px-3 py-1 text-xs transition-colors',
              schedule === p.value ? 'border-[var(--theme-accent)] text-accent' : 'border-line-subtle text-muted hover:border-line hover:text-fg',
            )}
          >
            {p.label}
          </button>
        ))}
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
          disabled={!ok || busy}
          onClick={() =>
            void onCreate({ name: name.trim(), schedule: schedule.trim(), prompt: prompt.trim() }).then((created) => {
              if (created) {
                setName('')
                setSchedule('')
                setPrompt('')
              }
            })
          }
        >
          {busy && <Loader2 size={13} className="animate-spin" />}
          {busy ? 'Creating' : 'Create job'}
        </Button>
        <span className="text-[11px] leading-snug text-muted">{scheduleHint()}</span>
      </div>
    </div>
  )
}

/** One agent's cron jobs: list + create + pause/run/delete. */
export function CronsPanel({ agentId, intro }: { agentId: string; intro?: string }) {
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
      {intro && <p className="text-xs leading-relaxed text-muted">{intro}</p>}
      {isLoading ? null : error ? (
        <EmptyState icon="◌" title="Schedules unavailable" hint={(error as Error).message} />
      ) : (data ?? []).length === 0 ? (
        <EmptyState icon={<CalendarClock size={22} />} title="Nothing scheduled" hint="Give it a recurring job below." />
      ) : (
        <ul className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
          {data!.map((j) => (
            <CronRow key={j.id} job={j} busy={busy} onAction={(a) => void act(j.id, a)} />
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

  const withJobs = agents.flatMap((a) => a.jobs.map((j) => ({ agent: a, job: j })))

  return (
    <Modal open onClose={onClose} title="Schedules · native Hermes crons" width="max-w-3xl">
      <div className="max-h-[70vh] space-y-5 overflow-y-auto pr-1">
        <p className="text-xs text-muted">
          Jobs run inside each agent's own scheduler. They keep firing even if Talaria is down. Fixed-time jobs
          created here are staggered 2&nbsp;minutes per agent so the fleet doesn't hit the models at once.
        </p>
        {isLoading ? (
          <div className="text-sm text-muted">Reading the fleet</div>
        ) : withJobs.length === 0 ? (
          <EmptyState icon={<CalendarClock size={22} />} title="Nothing scheduled anywhere" hint="Create the first job below." />
        ) : (
          <ul className="divide-y divide-line-subtle rounded-lg border border-line-subtle">
            {withJobs.map(({ agent, job }) => (
              <CronRow key={`${agent.id}-${job.id}`} job={job} agentLabel={agent.displayName} onAction={(a) => void act(agent.id, job.id, a)} />
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
          <CronForm onCreate={create} busy={busy}>
            <div className="flex flex-wrap gap-1.5">
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
