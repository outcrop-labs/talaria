import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { UserPlus, X } from 'lucide-react'
import { ChatView } from '@/components/chat/chat-view'
import { ConversationSidebar, type SidebarFailure } from '@/components/chat/conversation-sidebar'
import { RailSurface, Stage, StageHeader } from '@/components/app/surface'
import { PlanModal } from '@/components/chat/plan-modal'
import { PlanDoc, PlanDocSkeleton } from '@/components/chat/plan-doc'
import { userMentionInsert } from '@/components/chat/mentions'
import { UserPicker } from '@/components/app/user-picker'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Select } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { listQuery, QueryError } from '@/components/ui/query-state'
import { alert } from '@/components/ui/confirm'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { useTemplates } from '@/lib/templates'
import { useSession } from '@/lib/session'
import { useStickyAgent } from '@/lib/sticky-agent'
import { sharePlan, unsharePlan, usePlanMembers, useConversations, type Conversation } from '@/lib/conversations'

// Plan surface: think through the work with an agent, then draft tickets and
// send them to a board. A plan is a durable MULTIPLAYER conversation
// (kind='plan'): the owner shares it with teammates, everyone talks to the
// same agent and living document, presence shows who's here now.
export const Route = createFileRoute('/_app/plan')({
  component: PlanPage,
  // ?p=<planId> deep-links a shared plan (share notifications land here).
  validateSearch: (search: Record<string, unknown>): { p?: string } =>
    typeof search.p === 'string' && search.p ? { p: search.p } : {},
})

/** Members + presence + share, in the plan chat header. Owner shares/removes;
 *  a collaborator can leave. Green ring = viewing right now. */
function PlanMembers({ planId }: { planId: string }) {
  const { data, isLoading } = usePlanMembers(planId)
  const { data: session } = useSession()
  const qc = useQueryClient()
  const [adding, setAdding] = useState(false)
  const members = data?.members ?? []
  const active = new Set(data?.active ?? [])
  const isOwner = !!session?.id && session.id === members.find((m) => m.role === 'owner')?.userId
  const refresh = () => qc.invalidateQueries({ queryKey: ['plan-members', planId] })

  // Hold the avatar-stack footprint while members load, so the header row
  // (and the Draft-tickets button next to it) doesn't shift on resolve.
  if (isLoading)
    return (
      <div className="flex items-center gap-1.5">
        <div className="flex -space-x-1.5">
          <Skeleton className="h-6 w-6 rounded-full ring-2 ring-surface" />
          <Skeleton className="h-6 w-6 rounded-full ring-2 ring-surface" delay={0.12} />
        </div>
      </div>
    )

  return (
    <div className="flex items-center gap-1.5">
      <div className="flex -space-x-1.5">
        {members.map((m) => (
          <span key={m.userId} className="group relative" title={`${m.name ?? m.email}${m.role === 'owner' ? ' (owner)' : ''}${active.has(m.userId) ? ', here now' : ''}`}>
            <Avatar
              name={m.name ?? m.email ?? '?'}
              className={cn('h-6 w-6 text-[10px] ring-2 ring-surface', active.has(m.userId) && 'ring-success')}
            />
            {(isOwner && m.role !== 'owner') || (m.userId === session?.id && m.role === 'collaborator') ? (
              <button
                type="button"
                title={m.userId === session?.id ? 'Leave this plan' : `Remove ${m.name ?? m.email}`}
                onClick={() => void unsharePlan(planId, m.userId).then(refresh)}
                className="absolute -right-1 -top-1 hidden h-3.5 w-3.5 place-items-center rounded-full bg-raised text-muted shadow group-hover:grid hover:text-fg"
              >
                <X size={9} />
              </button>
            ) : null}
          </span>
        ))}
      </div>
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
              void sharePlan(planId, u.email)
                .then(refresh)
                .catch((e) => void alert({ title: 'Could not share', message: (e as Error).message }))
            }}
          />
        ) : (
          <button
            type="button"
            title="Share this plan with a teammate"
            onClick={() => setAdding(true)}
            className="grid h-6 w-6 place-items-center rounded-full border border-dashed border-line text-muted hover:text-fg"
          >
            <UserPlus size={12} />
          </button>
        ))}
    </div>
  )
}

function PlanPage() {
  const qc = useQueryClient()
  // Both reads keep their query object: the rail and the stage each render a
  // sentence ("No plans yet with this agent.", "No agents available.") that is
  // only true of a request that SUCCEEDED and came back empty.
  const fleetQuery = useAgents()
  const { data: fleet, isLoading: agentsLoading } = fleetQuery
  const agents = useMemo(() => fleet?.agents ?? [], [fleet])
  const conversationsQuery = useConversations('plan')
  const { data: conversations = [], isLoading: conversationsLoading } = conversationsQuery
  // Stale beats blank: a failed BACKGROUND refetch still has good data to show,
  // so only a failure with nothing behind it becomes a visible failure.
  const agentsFailure: SidebarFailure =
    fleetQuery.isError && fleetQuery.data === undefined
      ? { error: fleetQuery.error, retry: () => void fleetQuery.refetch() }
      : null
  const conversationsFailure: SidebarFailure =
    conversationsQuery.isError && conversationsQuery.data === undefined
      ? { error: conversationsQuery.error, retry: () => void conversationsQuery.refetch() }
      : null
  const [selectedAgent, pickAgent] = useStickyAgent('plan', agents)
  // The URL IS the plan selection (/plan?p=<id>) — linkable, back/forward-able.
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const selectedConversationId = search.p ?? null
  const setSelectedConversationId = (id: string | null, opts: { replace?: boolean } = {}) =>
    void navigate({ search: id ? { p: id } : {}, replace: opts.replace })
  // @mention the plan's MEMBERS — the people a mention will actually reach.
  // (Offering the whole org invited mentions that silently notified nobody.)
  // Tokens mirror the server's; a brand-new plan has only you, so it's inert.
  const { data: memberData } = usePlanMembers(selectedConversationId)
  const mentionables = useMemo(
    () =>
      (memberData?.members ?? [])
        .map((u) => ({ insert: userMentionInsert({ name: u.name, email: u.email }), label: u.name ?? u.email ?? u.userId, sub: u.email ?? undefined }))
        .filter((m) => m.insert),
    [memberData],
  )
  const [newChatSignal, setNewChatSignal] = useState(0)
  const [planOpen, setPlanOpen] = useState(false)
  // The template a NEW plan's living doc seeds from ('' = automatic: the plan
  // agent's bound plan template). Locked in when the first turn creates the plan.
  const [templateId, setTemplateId] = useState('')
  // Defaulted, a failed template read made the picker DISAPPEAR (the
  // `planTemplates.length > 0 &&` below), so a new plan silently seeded from
  // the agent default with no sign the choice had ever existed.
  const templatesList = listQuery(useTemplates(), { title: 'Could not load plan templates', variant: 'inline' })
  const templates = templatesList.rows
  const templatesLoading = templatesList.pending
  const planTemplates = useMemo(() => templates.filter((t) => t.kind === 'plan'), [templates])
  // Bumped when an agent turn lands; the doc pane syncs itself on it.
  const [turnSignal, setTurnSignal] = useState(0)

  const selectConversation = (c: Conversation) => {
    pickAgent(c.agentModel)
    setSelectedConversationId(c.id)
  }
  const selectAgent = (agentModel: string) => {
    pickAgent(agentModel)
    setSelectedConversationId(null)
    setNewChatSignal((n) => n + 1)
    setTemplateId('')
  }
  const newPlan = () => {
    if (selectedAgent) selectAgent(selectedAgent)
  }
  const onCreated = (id: string) => {
    setSelectedConversationId(id)
    void qc.invalidateQueries({ queryKey: ['conversations'] })
  }

  // An inbound ?p= may reference a plan whose AGENT isn't the sticky pick —
  // align the agent once the plan list resolves (URL itself stays put).
  useEffect(() => {
    if (!search.p) return
    const target = conversations.find((c) => c.id === search.p)
    if (target && target.agentModel !== selectedAgent) pickAgent(target.agentModel)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search.p, conversations])

  const current = agents.find((a) => a.id === selectedAgent)

  const selected = conversations.find((c) => c.id === selectedConversationId) ?? null

  return (
    <RailSurface>
      <ConversationSidebar
        agents={agents}
        conversations={conversations}
        selectedAgent={selectedAgent}
        selectedConversationId={selectedConversationId}
        agentsLoading={agentsLoading}
        conversationsLoading={conversationsLoading}
        agentsFailure={agentsFailure}
        conversationsFailure={conversationsFailure}
        onSelectAgent={selectAgent}
        onSelectConversation={selectConversation}
        onNewChat={newPlan}
      />

      <Stage
        header={
          selectedAgent && current ? (
            <StageHeader
              title={selected ? selected.title || 'Untitled plan' : 'New plan'}
              meta={`with ${current.label}`}
              actions={
                <div className="flex items-center gap-3">
                  {selectedConversationId ? (
                    <PlanMembers planId={selectedConversationId} />
                  ) : templatesLoading ? (
                    // Hold the template picker's spot so the header doesn't
                    // re-layout when templates land.
                    <Skeleton className="h-9 w-40 rounded-md" />
                  ) : templatesList.failed ? (
                    // The template read failed: say so inline. Falling through
                    // to the picker would hide the failure behind a list that
                    // only looks empty.
                    templatesList.notice
                  ) : (
                    planTemplates.length > 0 && (
                      <label className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted" title="The structure the living document starts from. Automatic uses the agent's bound plan template.">
                        Template
                        <Select size="sm" value={templateId} onChange={(e) => setTemplateId(e.target.value)} className="w-44">
                          <option value="">Automatic (agent default)</option>
                          {planTemplates.map((t) => (
                            <option key={t.id} value={t.id}>
                              {t.name}
                            </option>
                          ))}
                        </Select>
                      </label>
                    )
                  )}
                  <Button size="sm" variant="outline" disabled={!selectedConversationId} onClick={() => setPlanOpen(true)}>
                    Draft tickets
                  </Button>
                </div>
              }
            />
          ) : undefined
        }
      >
        {selectedAgent && current ? (
          <div className="flex h-full min-h-0">
            <div className="min-w-0 flex-1">
              <ChatView
                key={selectedAgent}
                agentModel={selectedAgent}
                agentLabel={current.label}
                tiers={current.tiers ?? []}
                agents={agents}
                onAgentChange={selectAgent}
                conversationId={selectedConversationId}
                newChatSignal={newChatSignal}
                onCreated={onCreated}
                kind="plan"
                templateId={templateId || null}
                fill
                mentionables={mentionables}
                onTurnComplete={() => setTurnSignal((n) => n + 1)}
              />
            </div>
            {/* The plan's living document — side by side from the FIRST
                keystroke, so the surface never re-layouts mid-thought. Before
                the conversation exists, the pane shows what will grow here. */}
            <div className="hidden min-w-0 basis-[44%] lg:flex">
              {selectedConversationId ? (
                <PlanDoc planId={selectedConversationId} planTitle={selected?.title ?? null} syncSignal={turnSignal} />
              ) : (
                <div className="flex min-w-0 flex-1 flex-col border-l border-line-subtle">
                  <div className="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
                    <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Plan document</span>
                  </div>
                  <div className="grid flex-1 place-items-center p-8 text-center">
                    <div className="max-w-56 font-sans text-xs leading-relaxed text-muted">
                      The living document builds here as you talk. {current.label} keeps it current, and you can edit it directly.
                    </div>
                  </div>
                </div>
              )}
            </div>
          </div>
        ) : agentsLoading ? (
          // The stage frame renders immediately while the fleet loads: the
          // same two-pane layout (chat column + 44% doc column) it stands in
          // for, so nothing pops or re-layouts when agents land.
          <div aria-hidden className="flex h-full min-h-0">
            <div className="min-w-0 flex-1 overflow-hidden p-6">
              <div className="mx-auto flex w-full max-w-[var(--chat-content-max-width)] flex-col gap-4">
                {/* Chat flattens onto the panel (spec §10) — the stand-in
                    blocks wear the same radius-8 panels the messages will. */}
                <Skeleton className="h-14 w-3/5 self-end rounded-lg" />
                <Skeleton className="h-24 w-4/5 rounded-lg" delay={0.12} />
                <Skeleton className="h-12 w-1/2 self-end rounded-lg" delay={0.24} />
                <Skeleton className="h-20 w-3/4 rounded-lg" delay={0.36} />
              </div>
            </div>
            <div className="hidden min-w-0 basis-[44%] border-l border-line-subtle lg:flex">
              <PlanDocSkeleton />
            </div>
          </div>
        ) : agentsFailure ? (
          // "No agents available." is a statement about the FLEET. When the
          // fleet read itself failed, the only honest thing to report is that.
          <div className="grid h-full place-items-center">
            <QueryError error={agentsFailure.error} title="Could not load your agents" onRetry={agentsFailure.retry} />
          </div>
        ) : (
          <div className="grid h-full place-items-center font-sans text-sm text-muted">No agents available.</div>
        )}
      </Stage>

      {selectedConversationId && current && planOpen && (
        <PlanModal
          open={planOpen}
          onClose={() => setPlanOpen(false)}
          draftUrl={`/api/plan/${selectedConversationId}/draft`}
          agents={[current]}
        />
      )}
    </RailSurface>
  )
}
