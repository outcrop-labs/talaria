import { cn } from '@/lib/cn'
import { Button } from '@/components/ui/button'
import { AgentPicker } from '@/components/chat/agent-picker'
import type { AgentModel } from '@/lib/agents'
import type { Conversation } from '@/lib/conversations'

// Right-aligned chat panel: an agent switcher on top, with the selected agent's
// conversations grouped beneath it. (The left rail is reserved for the main app
// menu.)
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
    <aside className="flex h-full w-72 shrink-0 flex-col border-l border-line-subtle bg-sidebar">
      <div className="space-y-2 border-b border-line-subtle p-4">
        <AgentPicker
          agents={agents}
          value={selectedAgent}
          onChange={onSelectAgent}
          loading={agentsLoading}
          fullWidth
        />
        <Button variant="outline" size="sm" className="w-full" onClick={onNewChat} disabled={!selectedAgent}>
          + New chat
        </Button>
      </div>

      <div className="flex-1 overflow-y-auto p-3">
        {agentConvs.length === 0 ? (
          <div className="px-2 py-6 text-center text-xs text-muted">No chats yet with this agent.</div>
        ) : (
          <ul className="space-y-0.5">
            {agentConvs.map((c) => (
              <li key={c.id}>
                <button
                  type="button"
                  onClick={() => onSelectConversation(c)}
                  className={cn(
                    'w-full truncate rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
                    c.id === selectedConversationId ? 'bg-card text-fg' : 'text-muted',
                  )}
                >
                  {c.title || 'Untitled'}
                  {c.role === 'collaborator' && c.ownerLabel && (
                    <span className="ml-1.5 text-[10px] text-muted">· {c.ownerLabel}</span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        )}
        {/* Plans shared WITH you ride other agents — surface them regardless
            of which agent is picked above. */}
        {sharedElsewhere.length > 0 && (
          <>
            <div className="mt-4 px-2 text-[10px] font-semibold uppercase tracking-wide text-muted">Shared with you</div>
            <ul className="mt-1 space-y-0.5">
              {sharedElsewhere.map((c) => (
                <li key={c.id}>
                  <button
                    type="button"
                    onClick={() => onSelectConversation(c)}
                    className={cn(
                      'w-full truncate rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
                      c.id === selectedConversationId ? 'bg-card text-fg' : 'text-muted',
                    )}
                  >
                    {c.title || 'Untitled'}
                    {c.ownerLabel && <span className="ml-1.5 text-[10px] text-muted">· {c.ownerLabel}</span>}
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </aside>
  )
}
