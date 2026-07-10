import { createFileRoute, Link } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ExternalLink, Loader2, Trash2 } from 'lucide-react'
import { RailSurface, Rail, Stage, StageHeader } from '@/components/app/surface'
import { Chip, DangerLink, StatusDot, type DotStatus } from '@/components/ui/chip'
import { SendButton } from '@/components/chat/composer-buttons'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { EmptyState } from '@/components/ui/empty-state'
import { Markdown } from '@/components/ui/markdown'
import { Panel } from '@/components/ui/panel'
import { Textarea } from '@/components/ui/textarea'
import { confirm } from '@/components/ui/confirm'
import { AgentPicker } from '@/components/chat/agent-picker'
import { useAgents } from '@/lib/agents'
import { useArtifact } from '@/lib/artifacts'
import { useSession } from '@/lib/session'
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
  const { data: runs = [] } = useResearchRuns()

  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [question, setQuestion] = useState('')
  const [mode, setMode] = useState<ResearchMode>('brief')
  const [agent, pickAgent] = useStickyAgent('research', agents)
  const [starting, setStarting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Deep link (?r=) from a completion notification.
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  useEffect(() => {
    if (search.r) {
      setSelectedId(search.r)
      void navigate({ search: {}, replace: true })
    }
  }, [search.r, navigate])

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
        <div className="mb-3 space-y-2.5 border-b border-line-subtle pb-3">
          <div className="flex items-end gap-2">
            <Textarea
              autoGrow
              rows={2}
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
              placeholder="What should we find out?"
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  void start()
                }
              }}
            />
            <SendButton onClick={() => void start()} disabled={starting || !question.trim() || !agent} title="Start research — Enter" />
          </div>
          <div className="grid grid-cols-3 gap-1.5">
            {(Object.keys(MODE_META) as ResearchMode[]).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={cn(
                  'rounded-lg border px-2 py-1.5 text-left transition-colors',
                  mode === m ? 'border-[color:var(--theme-accent)] bg-card' : 'border-line-subtle hover:bg-card',
                )}
                title={`${MODE_META[m].tagline}. The agent's expertise drives the search; every claim is cited.`}
              >
                <div className="text-xs font-semibold text-fg">{MODE_META[m].label}</div>
                <div className="text-[10px] text-muted">{MODE_META[m].eta}</div>
              </button>
            ))}
          </div>
          <AgentPicker agents={agents} value={agent} onChange={pickAgent} loading={agentsLoading} fullWidth />
          {starting && <div className="flex items-center gap-1.5 text-xs text-muted"><Loader2 size={12} className="animate-spin" /> starting</div>}
          {error && <div className="text-xs text-[color:var(--theme-danger)]">{error}</div>}
        </div>

        {runs.length === 0 ? (
          <div className="px-2 py-6 text-center text-xs text-muted">No research yet — ask something worth knowing.</div>
        ) : (
          <ul className="space-y-0.5">
            {runs.map((r) => (
              <li key={r.id}>
                <button
                  type="button"
                  onClick={() => setSelectedId(r.id)}
                  className={cn(
                    'group w-full rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-card',
                    selectedId === r.id ? 'bg-card' : '',
                  )}
                >
                  <div className="flex items-center gap-2">
                    <StatusDot status={STATUS_DOT[r.status]} pulse={r.status === 'running'} className="h-1.5 w-1.5" />
                    <span className="min-w-0 flex-1 truncate text-sm text-fg">{r.question}</span>
                    {canDelete(r) && (
                      <Trash2
                        size={13}
                        className="hidden shrink-0 text-muted hover:text-[color:var(--theme-danger)] group-hover:block"
                        onClick={(e) => {
                          e.stopPropagation()
                          void remove(r)
                        }}
                      />
                    )}
                  </div>
                  <div className="mt-0.5 flex items-center gap-2 pl-3.5 text-[10px] text-muted">
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
              title={selected.question}
              meta={`${MODE_META[selected.mode].label} · ${selected.agentModel}`}
              actions={canDelete(selected) ? <DangerLink onClick={() => void remove(selected)}>Remove</DangerLink> : undefined}
            />
          ) : undefined
        }
      >
        <div className="h-full overflow-y-auto">
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
      </Stage>
    </RailSurface>
  )
}

function RunView({ runId }: { runId: string }) {
  const { data } = useResearchRun(runId)
  const run = data?.run
  const { data: artifact } = useArtifact(run?.artifactId ?? null)

  if (!run) return <div className="p-8 text-sm text-muted">Loading</div>

  return (
    <div className="mx-auto max-w-3xl space-y-4 p-8">
      <div className="flex items-center gap-2 text-xs text-muted">
        <Avatar name={run.agentModel} className="h-6 w-6 text-[10px]" />
        <span>by {run.requestedBy}</span>
        {run.stats.sources !== undefined && <span>· {run.stats.sources} sources ({run.stats.cited} cited)</span>}
        <span className="ml-auto" />
        {run.artifactId && (
          <Link to="/artifacts" className="flex shrink-0 items-center gap-1 text-xs text-muted hover:text-fg">
            Open in Artifacts <ExternalLink size={12} />
          </Link>
        )}
      </div>

      {(run.status === 'queued' || run.status === 'running') && (
        <Panel className="flex items-center gap-3">
          <Loader2 size={16} className="animate-spin text-accent" />
          <div className="min-w-0">
            <div className="text-sm text-fg">{run.status === 'queued' ? 'Queued' : 'Researching'}</div>
            {run.phase && <div className="truncate text-xs text-muted">{run.phase}</div>}
          </div>
        </Panel>
      )}
      {run.status === 'error' && (
        <Panel>
          <div className="text-sm" style={{ color: 'var(--theme-danger)' }}>
            {run.error ?? 'The run failed.'}
          </div>
        </Panel>
      )}

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
