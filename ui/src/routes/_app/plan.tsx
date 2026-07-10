import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { UserPlus, X } from 'lucide-react'
import { ChatView } from '@/components/chat/chat-view'
import { ConversationSidebar } from '@/components/chat/conversation-sidebar'
import { RailSurface, Stage, StageHeader } from '@/components/app/surface'
import { PlanModal } from '@/components/chat/plan-modal'
import { PlanDoc } from '@/components/chat/plan-doc'
import { userMentionInsert } from '@/components/chat/mentions'
import { UserPicker } from '@/components/app/user-picker'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { alert } from '@/components/ui/confirm'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { useSession } from '@/lib/session'
import { useStickyAgent } from '@/lib/sticky-agent'
import { sharePlan, unsharePlan, usePlanMembers, useConversations, type Conversation } from '@/lib/conversations'
import { useUsers } from '@/lib/users'

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
  const { data } = usePlanMembers(planId)
  const { data: session } = useSession()
  const qc = useQueryClient()
  const [adding, setAdding] = useState(false)
  const members = data?.members ?? []
  const active = new Set(data?.active ?? [])
  const isOwner = !!session?.id && session.id === members.find((m) => m.role === 'owner')?.userId
  const refresh = () => qc.invalidateQueries({ queryKey: ['plan-members', planId] })

  return (
    <div className="flex items-center gap-1.5">
      <div className="flex -space-x-1.5">
        {members.map((m) => (
          <span key={m.userId} className="group relative" title={`${m.name ?? m.email}${m.role === 'owner' ? ' (owner)' : ''}${active.has(m.userId) ? ' — here now' : ''}`}>
            <Avatar
              name={m.name ?? m.email ?? '?'}
              className={cn('h-6 w-6 text-[10px] ring-2 ring-surface', active.has(m.userId) && 'ring-[color:var(--theme-success,#22c55e)]')}
            />
            {(isOwner && m.role !== 'owner') || (m.userId === session?.id && m.role === 'collaborator') ? (
              <button
                type="button"
                title={m.userId === session?.id ? 'Leave this plan' : `Remove ${m.name ?? m.email}`}
                onClick={() => void unsharePlan(planId, m.userId).then(refresh)}
                className="absolute -right-1 -top-1 hidden h-3.5 w-3.5 place-items-center rounded-full bg-card text-muted shadow group-hover:grid hover:text-fg"
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
  const { data: fleet, isLoading: agentsLoading } = useAgents()
  const agents = useMemo(() => fleet?.agents ?? [], [fleet])
  const { data: conversations = [] } = useConversations('plan')
  // @mention teammates while planning — they're notified once they can read
  // the plan's document (i.e. after it's shared). Tokens mirror the server's.
  const { data: users = [] } = useUsers()
  const mentionables = useMemo(
    () =>
      users
        .map((u) => ({ insert: userMentionInsert(u), label: u.name ?? u.email ?? u.id, sub: u.email ?? undefined }))
        .filter((m) => m.insert),
    [users],
  )

  const [selectedAgent, pickAgent] = useStickyAgent('plan', agents)
  const [selectedConversationId, setSelectedConversationId] = useState<string | null>(null)
  const [newChatSignal, setNewChatSignal] = useState(0)
  const [planOpen, setPlanOpen] = useState(false)

  const selectConversation = (c: Conversation) => {
    pickAgent(c.agentModel)
    setSelectedConversationId(c.id)
  }
  const selectAgent = (agentModel: string) => {
    pickAgent(agentModel)
    setSelectedConversationId(null)
    setNewChatSignal((n) => n + 1)
  }
  const newPlan = () => {
    if (selectedAgent) selectAgent(selectedAgent)
  }
  const onCreated = (id: string) => {
    setSelectedConversationId(id)
    void qc.invalidateQueries({ queryKey: ['conversations'] })
  }

  // A ?p= deep link (share notification) selects that plan once it's loaded.
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  useEffect(() => {
    if (!search.p) return
    const target = conversations.find((c) => c.id === search.p)
    if (target) {
      selectConversation(target)
      void navigate({ search: {}, replace: true })
    }
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
                  {selectedConversationId && <PlanMembers planId={selectedConversationId} />}
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
                conversationId={selectedConversationId}
                newChatSignal={newChatSignal}
                onCreated={onCreated}
                kind="plan"
                fill
                mentionables={mentionables}
              />
            </div>
            {/* The plan's living document — side by side from the FIRST
                keystroke, so the surface never re-layouts mid-thought. Before
                the conversation exists, the pane shows what will grow here. */}
            <div className="hidden min-w-0 basis-[44%] lg:flex">
              {selectedConversationId ? (
                <PlanDoc planId={selectedConversationId} planTitle={selected?.title ?? null} />
              ) : (
                <div className="flex min-w-0 flex-1 flex-col border-l border-line-subtle">
                  <div className="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
                    <span className="text-[10px] font-semibold uppercase tracking-wide text-muted">Plan document</span>
                  </div>
                  <div className="grid flex-1 place-items-center p-8 text-center">
                    <div className="max-w-56 text-xs leading-relaxed text-muted">
                      The living document builds here as you talk — {current.label} keeps it current, and you can edit it directly.
                    </div>
                  </div>
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="grid h-full place-items-center text-sm text-muted">
            {agentsLoading ? 'Loading the fleet' : 'No agents available.'}
          </div>
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
