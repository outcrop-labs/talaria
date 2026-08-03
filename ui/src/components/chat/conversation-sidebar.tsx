// The plan rail — LEFT, on the shared Rail primitives (agent picker up top,
// this agent's plans below, plans shared with you at the end).
import { Plus } from 'lucide-react'
import { useHasPerm } from '@/lib/session'
import { AgentPicker } from '@/components/chat/agent-picker'
import { IconButton } from '@/components/ui/icon-button'
import { Rail, RailRow, RailSection } from '@/components/app/surface'
import { SkeletonRows } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import type { AgentModel } from '@/lib/agents'
import type { Conversation } from '@/lib/conversations'

/** A read that FAILED, handed down from the route that owns the query. This
 *  component only ever sees already-defaulted arrays (`= []`), so an outage and
 *  a genuinely empty list arrive here looking exactly alike — the caller has to
 *  tell it which one it is, or "No plans yet with this agent." goes out over a
 *  500 and a person's plans read as never having existed. */
export type SidebarFailure = { error: unknown; retry: () => void } | null

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
              <span className="min-w-0 flex-1 truncate">{c.title || 'Untitled'}</span>
              {c.role === 'collaborator' && c.ownerLabel && (
                <span className="shrink-0 text-[10px] text-muted">{c.ownerLabel}</span>
              )}
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
                <span className="min-w-0 flex-1 truncate">{c.title || 'Untitled'}</span>
                {c.ownerLabel && <span className="shrink-0 text-[10px] text-muted">{c.ownerLabel}</span>}
              </RailRow>
            ))}
          </RailSection>
        </div>
      )}
    </Rail>
  )
}
