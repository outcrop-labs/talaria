// The plan rail — LEFT, on the shared Rail primitives (agent picker up top,
// this agent's plans below, plans shared with you at the end).
import { Plus } from 'lucide-react'
import { AgentPicker } from '@/components/chat/agent-picker'
import { IconButton } from '@/components/ui/icon-button'
import { Rail, RailRow, RailSection } from '@/components/app/surface'
import type { AgentModel } from '@/lib/agents'
import type { Conversation } from '@/lib/conversations'

export function ConversationSidebar({
  agents,
  conversations,
  selectedAgent,
  selectedConversationId,
  agentsLoading,
  onSelectAgent,
  onSelectConversation,
  onNewChat,
}: {
  agents: AgentModel[]
  conversations: Conversation[]
  selectedAgent: string | null
  selectedConversationId: string | null
  agentsLoading?: boolean
  onSelectAgent: (agentModel: string) => void
  onSelectConversation: (conv: Conversation) => void
  onNewChat: () => void
}) {
  const agentConvs = conversations.filter((c) => c.agentModel === selectedAgent)
  const sharedElsewhere = conversations.filter((c) => c.role === 'collaborator' && c.agentModel !== selectedAgent)

  return (
    <Rail
      title="Plan"
      actions={<IconButton size="sm" title="New plan — think it through, then draft tickets" onClick={onNewChat} disabled={!selectedAgent}><Plus size={15} /></IconButton>}
    >
      <div className="mb-3">
        <AgentPicker agents={agents} value={selectedAgent} onChange={onSelectAgent} loading={agentsLoading} fullWidth />
      </div>

      {agentConvs.length === 0 ? (
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
