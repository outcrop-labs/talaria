import { createFileRoute, Link } from '@tanstack/react-router'
import { Skeleton } from '@/components/ui/skeleton'
import { useMemo, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ExternalLink, Gauge, Trash2, UserPlus, X } from 'lucide-react'
import { GeneratingDots, GeneratingHelix } from '@/components/ui/generating'
import { RailSurface, Rail, Stage, StageHeader } from '@/components/app/surface'
import { Chip, DangerLink, StatusDot, type DotStatus } from '@/components/ui/chip'
import { ComposerPicker } from '@/components/chat/composer-picker'
import { SendButton } from '@/components/chat/composer-buttons'
import { AgentPicker } from '@/components/chat/agent-picker'
import { useContextMenu, copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu'
import { KeyHint } from '@/components/ui/kbd'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { EmptyState } from '@/components/ui/empty-state'
import { Markdown } from '@/components/ui/markdown'
import { Panel } from '@/components/ui/panel'
import { Textarea } from '@/components/ui/textarea'
import { alert, confirm } from '@/components/ui/confirm'
import { UserPicker } from '@/components/app/user-picker'
import { useAgents } from '@/lib/agents'
import { useArtifact } from '@/lib/artifacts'
import { useHasPerm, useSession } from '@/lib/session'
import { useStickyAgent } from '@/lib/sticky-agent'
import { relativeTime } from '@/lib/fleet'
import {
  deleteResearch,
  MODE_META,
  startResearch,
  useResearchRun,
  useResearchRuns,
  type ResearchMode,
  type ResearchRun,
} from '@/lib/research'

// Research — Perplexity-grade cited research, run by YOUR agents. Ask a
// question, pick a depth (Recon / Brief / Expedition) and whose expertise
// should drive it; the pipeline runs server-side and lands an org-visible,
// fully cited report document. Everything indexes into the activity brain so
// chats, plans, and boards can pull from it later.
export const Route = createFileRoute('/_app/research')({
  component: ResearchPage,
  // ?r=<runId> deep-links a run (completion notifications land here).
  validateSearch: (search: Record<string, unknown>): { r?: string } =>
    typeof search.r === 'string' && search.r ? { r: search.r } : {},
})

const STATUS_DOT: Record<ResearchRun['status'], DotStatus> = {
  queued: 'idle',
  running: 'accent',
  done: 'ok',
  error: 'danger',
}

function ResearchPage() {
  const qc = useQueryClient()
  const { data: session } = useSession()
  const { data: fleet, isLoading: agentsLoading } = useAgents()
  const agents = useMemo(() => fleet?.agents ?? [], [fleet])
  const { data: runs = [], isLoading: runsLoading } = useResearchRuns()

  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  // URL is the selection: /research?r=<runId> IS the selected run.
  const selectedId = search.r ?? null
  const setSelectedId = (id: string | null) => void navigate({ search: id ? { r: id } : {} })
  const [question, setQuestion] = useState('')
  const mayRun = useHasPerm('research.run')
  const [mode, setMode] = useState<ResearchMode>('brief')
  const [agent, pickAgent] = useStickyAgent('research', agents)
  const [starting, setStarting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { openMenu, menu } = useContextMenu()


  const start = async () => {
    if (!question.trim() || !agent) return
    setStarting(true)
    setError(null)
    try {
      const run = await startResearch(question.trim(), mode, agent)
      setQuestion('')
      setSelectedId(run.id)
      void qc.invalidateQueries({ queryKey: ['research-runs'] })
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setStarting(false)
    }
  }

  const remove = async (run: ResearchRun) => {
    if (!(await confirm({ title: 'Remove run', message: `Remove "${run.question.slice(0, 80)}" from the list? The report document (if any) stays in Artifacts.`, confirmLabel: 'Remove' }))) return
    await deleteResearch(run.id)
    if (selectedId === run.id) setSelectedId(null)
    void qc.invalidateQueries({ queryKey: ['research-runs'] })
  }

  const canDelete = (run: ResearchRun) => run.ownerUserId === session?.id || session?.role === 'admin'

  const selected = runs.find((r) => r.id === selectedId) ?? null

  return (
    <RailSurface>
      <Rail title="Research">
        <div className="mb-3">
          <AgentPicker agents={agents} value={agent} onChange={pickAgent} loading={agentsLoading} fullWidth />
        </div>
        {runsLoading ? (
          // Rail-row-shaped placeholders while the run list loads; the empty
          // state only appears once the query has RESOLVED empty.
          <div aria-hidden className="space-y-0.5">
            {['88%', '72%', '95%', '60%', '80%'].map((w, i) => (
              <div key={i} className="px-2 py-1.5">
                <div style={{ width: w }}>
                  <Skeleton className="h-3 w-full rounded-full" delay={i * 0.12} />
                </div>
                <div className="mt-1.5 w-2/5 pl-3.5">
                  <Skeleton className="h-2.5 w-full rounded-full" delay={i * 0.12 + 0.06} />
                </div>
              </div>
            ))}
          </div>
        ) : runs.length === 0 ? (
          <EmptyState variant="compact" icon="◎" title="No research yet." hint="Ask something worth knowing." />
        ) : (
          <ul className="space-y-0.5">
            {runs.map((r) => (
              <li key={r.id}>
                <button
                  type="button"
                  onClick={() => setSelectedId(r.id)}
                  onContextMenu={(e) =>
                    openMenu(e, [
                      { label: 'Open', onSelect: () => setSelectedId(r.id) },
                      { label: 'Copy link', onSelect: () => copyAppLink(`/research?r=${r.id}`) },
                      ...(canDelete(r)
                        ? (['sep', { label: 'Remove', danger: true, onSelect: () => void remove(r) }] as ContextMenuEntry[])
                        : []),
                    ])
                  }
                  className={cn(
                    'group w-full rounded-md px-2 py-1.5 text-left transition-colors hover:bg-hover',
                    selectedId === r.id ? 'bg-card' : '',
                  )}
                >
                  <div className="flex items-center gap-2">
                    <StatusDot status={STATUS_DOT[r.status]} pulse={r.status === 'running'} className="h-1.5 w-1.5" />
                    <span className="min-w-0 flex-1 truncate text-sm text-fg">{r.title ?? r.question}</span>
                    {canDelete(r) && (
                      <Trash2
                        size={13}
                        className="hidden shrink-0 text-muted transition-colors hover:text-danger group-hover:block"
                        onClick={(e) => {
                          e.stopPropagation()
                          void remove(r)
                        }}
                      />
                    )}
                  </div>
                  {/* §10 session-row meta: 10px mono chrome voice. */}
                  <div className="mt-0.5 flex items-center gap-2 pl-3.5 font-mono text-[10px] tracking-[0.05em] text-muted">
                    <Chip>{MODE_META[r.mode].label}</Chip>
                    <span className="truncate">{r.requestedBy}</span>
                    <span className="ml-auto shrink-0">{relativeTime(r.createdAt)}</span>
                  </div>
                </button>
              </li>
            ))}
          </ul>
        )}
      </Rail>

      <Stage
        header={
          selected ? (
            <StageHeader
              title={selected.title ?? selected.question}
              meta={`${MODE_META[selected.mode].label} · ${selected.agentModel}`}
              actions={canDelete(selected) ? <DangerLink onClick={() => void remove(selected)}>Remove</DangerLink> : undefined}
            />
          ) : undefined
        }
      >
        <div className="flex h-full min-h-0 flex-col">
          <div className="min-h-0 flex-1 overflow-y-auto">
            {selectedId ? (
              <RunView runId={selectedId} />
            ) : (
              <div className="grid h-full place-items-center p-8">
                <EmptyState
                  icon="◎"
                  title="Research"
                  hint="Recon answers fast; Brief maps a topic; Expedition goes deep. Reports are cited, org-visible documents your agents can retrieve later."
                />
              </div>
            )}
          </div>

          {/* The ask lives where every other conversation input lives — the
              bottom of the stage. Depth and acting agent ride along as
              composer pills, tier-picker style. */}
          <div className="px-6 pb-6 pt-2">
            {/* §7 composer anatomy: panel body, STRONG hairline, radius 8,
                p-2, matte float shadow — with the prompt in a ground inset. */}
            <div className="mx-auto w-full max-w-[var(--chat-content-max-width)] rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)]">
              <div className="flex items-end gap-2">
                <div className="relative min-w-0 flex-1 rounded-md border border-line bg-surface px-2 py-1">
                  <Textarea
                    autoGrow
                    rows={1}
                    value={question}
                    disabled={!mayRun}
                    onChange={(e) => setQuestion(e.target.value)}
                    placeholder={mayRun ? 'What should we find out?' : 'You don’t have permission to run research'}
                    className="max-h-40 min-h-[2.75rem] border-0 bg-transparent pr-12 focus:border-0"
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && !e.shiftKey) {
                        e.preventDefault()
                        void start()
                      }
                    }}
                  />
                  {/* §7 signature affordance: gold send tile inside the well,
                      top-right — mouse/touch path to start a run. */}
                  <SendButton
                    className="absolute right-2 top-2"
                    title="Start (⏎)"
                    enabled={mayRun && !starting && !!question.trim() && !!agent}
                    onClick={() => void start()}
                  />
                </div>
                <KeyHint keys="⏎" label="start" visible={!!question.trim() && !!agent} className="self-end mb-3" />
                <ComposerPicker
                  icon={Gauge}
                  value={mode}
                  onChange={(m) => setMode(m as ResearchMode)}
                  title="Research depth"
                  menuLabel="Depth"
                  options={(Object.keys(MODE_META) as ResearchMode[]).map((m) => ({
                    value: m,
                    label: MODE_META[m].label,
                    sub: `${MODE_META[m].tagline} · ${MODE_META[m].eta}`,
                  }))}
                />
                {starting && (
                  <span className="grid h-9 w-9 shrink-0 place-items-center self-end mb-1"><GeneratingDots /></span>
                )}
              </div>
              {error && <div className="px-2 pb-1 pt-1 text-xs text-danger">{error}</div>}
            </div>
          </div>
        </div>
      </Stage>
      {menu}
    </RailSurface>
  )
}

/** Report-document placeholder: a panel of prose bars, shaped like the
 *  Markdown report it stands in for. */
function ReportSkeleton() {
  const widths = ['66%', '100%', '94%', '88%', '97%', '82%', '91%', '100%', '86%', '95%', '78%', '58%']
  return (
    <Panel aria-hidden>
      <div className="space-y-3">
        {widths.map((w, i) => (
          <div key={i} style={{ width: w }}>
            <Skeleton className={i === 0 ? 'h-4 w-full rounded-full' : 'h-2.5 w-full rounded-full'} delay={i * 0.12} />
          </div>
        ))}
      </div>
    </Panel>
  )
}

/** Members + share, in the run header. Mirrors plan sharing: the owner adds
 *  teammates (they get the run AND its report), a collaborator can leave. */
function ResearchMembers({ runId }: { runId: string }) {
  const { data: session } = useSession()
  const qc = useQueryClient()
  const [adding, setAdding] = useState(false)
  const { data } = useQuery({
    queryKey: ['research-members', runId],
    queryFn: async (): Promise<{ members: Array<{ userId: string; name: string | null; email: string | null; role: 'owner' | 'collaborator' }> }> => {
      const r = await fetch(`/api/research/${runId}/members`, { credentials: 'same-origin' })
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
  })
  const members = data?.members ?? []
  const isOwner = !!session?.id && session.id === members.find((m) => m.role === 'owner')?.userId
  const refresh = () => qc.invalidateQueries({ queryKey: ['research-members', runId] })
  const remove = (userId: string) =>
    fetch(`/api/research/${runId}/members`, {
      method: 'DELETE',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ userId }),
    }).then(refresh)
  // Org-wide runs (no owner) have nothing to share.
  if (members.length === 0 || !members.some((m) => m.role === 'owner')) return null

  return (
    <span className="flex items-center gap-1.5">
      <span className="flex -space-x-1.5">
        {members.map((m) => (
          <span key={m.userId} className="group relative" title={`${m.name ?? m.email}${m.role === 'owner' ? ' (owner)' : ''}`}>
            <Avatar name={m.name ?? m.email ?? '?'} className="h-6 w-6 text-[10px] ring-2 ring-surface" />
            {(isOwner && m.role !== 'owner') || (m.userId === session?.id && m.role === 'collaborator') ? (
              <button
                type="button"
                title={m.userId === session?.id ? 'Leave this research' : `Remove ${m.name ?? m.email}`}
                onClick={() => void remove(m.userId)}
                className="absolute -right-1 -top-1 hidden h-3.5 w-3.5 place-items-center rounded-full bg-card text-muted shadow group-hover:grid hover:text-fg"
              >
                <X size={9} />
              </button>
            ) : null}
          </span>
        ))}
      </span>
      {isOwner &&
        (adding ? (
          <UserPicker
            size="sm"
            className="w-48"
            placeholder="Share with"
            exclude={members.map((m) => m.userId)}
            onPick={(u) => {
              setAdding(false)
              if (!u.email) return
              void fetch(`/api/research/${runId}/members`, {
                method: 'POST',
                credentials: 'same-origin',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ email: u.email }),
              }).then(async (r) => {
                if (!r.ok) {
                  const e = ((await r.json().catch(() => ({}))) as { error?: string }).error
                  void alert({ title: 'Could not share', message: e ?? 'share failed' })
                }
                refresh()
              })
            }}
          />
        ) : (
          <button
            type="button"
            title="Share this research with a teammate"
            onClick={() => setAdding(true)}
            className="grid h-6 w-6 place-items-center rounded-full border border-dashed border-line text-muted hover:text-fg"
          >
            <UserPlus size={12} />
          </button>
        ))}
    </span>
  )
}

function RunView({ runId }: { runId: string }) {
  const { data } = useResearchRun(runId)
  const run = data?.run
  const { data: artifact, isLoading: artifactLoading } = useArtifact(run?.artifactId ?? null)

  if (!run)
    return (
      // The shape of the run view to come: meta row, status panel, report panel.
      <div aria-hidden className="mx-auto w-full max-w-[var(--chat-content-max-width)] space-y-4 p-8">
        <div className="flex items-center gap-2">
          <Skeleton className="h-6 w-6 shrink-0 rounded-full" />
          <Skeleton className="h-2.5 w-32 rounded-full" delay={0.12} />
          <Skeleton className="h-2.5 w-24 rounded-full" delay={0.24} />
        </div>
        <Panel className="flex items-center gap-3">
          <Skeleton className="h-4 w-4 shrink-0 rounded-full" />
          <div className="min-w-0 flex-1 space-y-2">
            <Skeleton className="h-3 w-28 rounded-full" delay={0.12} />
            <Skeleton className="h-2.5 w-48 rounded-full" delay={0.24} />
          </div>
        </Panel>
        <ReportSkeleton />
      </div>
    )

  return (
    <div className="mx-auto w-full max-w-[var(--chat-content-max-width)] space-y-4 p-8">
      {/* Run meta line — chrome voice: 11px mono muted (spec §2). */}
      <div className="flex items-center gap-2 font-mono text-[11px] text-muted">
        <Avatar name={run.agentModel} className="h-6 w-6 text-[10px]" />
        <span>by {run.requestedBy}</span>
        {run.stats.sources !== undefined && <span>· {run.stats.sources} sources ({run.stats.cited} cited)</span>}
        <span className="ml-auto" />
        <ResearchMembers runId={runId} />
        {run.artifactId && (
          <Link to="/artifacts" className="flex shrink-0 items-center gap-1 text-muted transition-colors hover:text-fg">
            Open in Artifacts <ExternalLink size={12} />
          </Link>
        )}
      </div>

      {(run.status === 'queued' || run.status === 'running') && (
        <Panel className="flex items-center gap-3">
          <GeneratingHelix />
          <div className="min-w-0">
            <div className="text-sm text-fg">{run.status === 'queued' ? 'Queued' : 'Researching'}</div>
            {run.phase && <div className="truncate text-xs text-muted">{run.phase}</div>}
          </div>
        </Panel>
      )}
      {run.status === 'error' && (
        <Panel>
          <div className="text-sm text-danger">{run.error ?? 'The run failed.'}</div>
        </Panel>
      )}

      {/* The report body holds its shape while the artifact fetch is in
          flight — no null-then-pop when the document lands. */}
      {run.artifactId && !artifact && artifactLoading && <ReportSkeleton />}
      {artifact && (
        <Panel>
          <Markdown className="prose-sm">{artifact.body}</Markdown>
        </Panel>
      )}

      {data && data.sources.length > 0 && run.status === 'done' && !artifact && (
        <Panel>
          <div className="text-xs text-muted">Report document is loading</div>
        </Panel>
      )}
    </div>
  )
}
