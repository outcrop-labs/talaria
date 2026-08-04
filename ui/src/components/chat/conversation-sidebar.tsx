// The plan rail — LEFT, on the shared Rail primitives (agent picker up top,
// this agent's plans below, plans shared with you at the end). Rows follow
// the Gentle dew session-list pattern (spec §10): status dot, 13px sans
// title, right-aligned mono meta/time; the active row carries the gold dot.
import { Plus } from 'lucide-react'
import { cn } from '@/lib/cn'
import { useHasPerm } from '@/lib/session'
import { AgentPicker } from '@/components/chat/agent-picker'
import { IconButton } from '@/components/ui/icon-button'
import { Rail, RailRow, RailSection } from '@/components/app/surface'
import { SkeletonRows } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { relativeTime } from '@/lib/fleet'
import type { AgentModel } from '@/lib/agents'
import type { Conversation } from '@/lib/conversations'

/** A read that FAILED, handed down from the route that owns the query. This
 *  component only ever sees already-defaulted arrays (`= []`), so an outage and
 *  a genuinely empty list arrive here looking exactly alike — the caller has to
 *  tell it which one it is, or "No plans yet with this agent." goes out over a
 *  500 and a person's plans read as never having existed. */
export type SidebarFailure = { error: unknown; retry: () => void } | null

/** §10 session row content: [status dot] [13px title] [mono owner] [mono time].
 *  Shared: the Plan rail and the Comms agent-thread rows are both session
 *  lists — one anatomy, one component. */
export function SessionRowBody({ conv, active }: { conv: Conversation; active: boolean }) {
  // Semantic first (spec §1): failure orange, working green; the active row's
  // dot reads gold, idle rows stay dim ink.
  const dot = conv.failed
    ? 'var(--theme-danger)'
    : conv.working
      ? 'var(--theme-success)'
      : active
        ? 'var(--theme-accent)'
        : 'var(--theme-ink-dim)'
  return (
    <>
      <span
        aria-hidden
        // A working session is live background state — MONITOR BREATHE on the
        // ambient budget (spec §9); idle/failed dots stay still.
        className={cn('h-[6px] w-[6px] shrink-0 rounded-full', conv.working && !conv.failed && 'gd-breathe')}
        style={{ backgroundColor: dot }}
      />
      <span className="min-w-0 flex-1 truncate font-sans text-[13px]">{conv.title || 'Untitled'}</span>
      {conv.ownerLabel && (
        <span className="max-w-16 shrink-0 truncate font-mono text-[10px] tracking-[0.05em] text-muted">
          {conv.ownerLabel}
        </span>
      )}
      <span className="shrink-0 font-mono text-[10px] tracking-[0.05em] text-ink-dim">
        {relativeTime(conv.updatedAt)}
      </span>
    </>
  )
}

export function ConversationSidebar({
  agents,
  conversations,
  selectedAgent,
  selectedConversationId,
  agentsLoading,
  conversationsLoading,
  agentsFailure = null,
  conversationsFailure = null,
  onSelectAgent,
  onSelectConversation,
  onNewChat,
}: {
  agents: AgentModel[]
  conversations: Conversation[]
  selectedAgent: string | null
  selectedConversationId: string | null
  agentsLoading?: boolean
  conversationsLoading?: boolean
  agentsFailure?: SidebarFailure
  conversationsFailure?: SidebarFailure
  onSelectAgent: (agentModel: string) => void
  onSelectConversation: (conv: Conversation) => void
  onNewChat: () => void
}) {
  const agentConvs = conversations.filter((c) => c.agentModel === selectedAgent)
  const sharedElsewhere = conversations.filter((c) => c.role === 'collaborator' && c.agentModel !== selectedAgent)

  return (
    <Rail
      title="Plan"
      actions={
        useHasPerm('plans.create') ? (
          <IconButton size="sm" title="New plan: think it through, then draft tickets" onClick={onNewChat} disabled={!selectedAgent}><Plus size={15} /></IconButton>
        ) : null
      }
    >
      <div className="mb-3">
        <AgentPicker agents={agents} value={selectedAgent} onChange={onSelectAgent} loading={agentsLoading} fullWidth />
        {/* An empty picker is indistinguishable from a fleet you don't have. */}
        {agentsFailure && (
          <QueryError
            variant="inline"
            className="mt-1.5 px-2"
            error={agentsFailure.error}
            title="Could not load your agents"
            onRetry={agentsFailure.retry}
          />
        )}
      </div>

      {conversationsLoading ? (
        <SkeletonRows rows={5} className="px-2 pt-1.5" />
      ) : conversationsFailure ? (
        <QueryError
          variant="compact"
          error={conversationsFailure.error}
          title="Could not load your plans"
          onRetry={conversationsFailure.retry}
        />
      ) : agentConvs.length === 0 ? (
        <div className="px-2 py-6 text-center text-xs text-muted">No plans yet with this agent.</div>
      ) : (
        <ul className="space-y-0.5">
          {agentConvs.map((c) => (
            <RailRow key={c.id} active={c.id === selectedConversationId} onClick={() => onSelectConversation(c)}>
              <SessionRowBody conv={c} active={c.id === selectedConversationId} />
            </RailRow>
          ))}
        </ul>
      )}

      {/* Plans shared WITH you ride other agents — always visible. */}
      {sharedElsewhere.length > 0 && (
        <div className="mt-4">
          <RailSection label="Shared with you">
            {sharedElsewhere.map((c) => (
              <RailRow key={c.id} active={c.id === selectedConversationId} onClick={() => onSelectConversation(c)}>
                <SessionRowBody conv={c} active={c.id === selectedConversationId} />
              </RailRow>
            ))}
          </RailSection>
        </div>
      )}
    </Rail>
  )
}
