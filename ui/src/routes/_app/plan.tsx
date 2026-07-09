import { createFileRoute } from '@tanstack/react-router'
import { useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ChatView } from '@/components/chat/chat-view'
import { ConversationSidebar } from '@/components/chat/conversation-sidebar'
import { PlanModal } from '@/components/chat/plan-modal'
import { PlanDoc } from '@/components/chat/plan-doc'
import { Button } from '@/components/ui/button'
import { useAgents } from '@/lib/agents'
import { useStickyAgent } from '@/lib/sticky-agent'
import { useConversations, type Conversation } from '@/lib/conversations'

// Plan surface: think through the work with an agent, then draft tickets and
// send them to a board. A plan is a durable conversation (kind='plan') — same
// stream as chat, plus the "Draft tickets" action.
export const Route = createFileRoute('/_app/plan')({
  component: PlanPage,
})

function PlanPage() {
  const qc = useQueryClient()
  const { data: fleet, isLoading: agentsLoading } = useAgents()
  const agents = useMemo(() => fleet?.agents ?? [], [fleet])
  const { data: conversations = [] } = useConversations('plan')

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

  const current = agents.find((a) => a.id === selectedAgent)

  return (
    <div className="flex h-full min-h-0">
      <main className="flex min-h-0 flex-1">
        {selectedAgent && current ? (
          <>
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
                headerAction={
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={!selectedConversationId}
                    onClick={() => setPlanOpen(true)}
                  >
                    Draft tickets
                  </Button>
                }
              />
            </div>
            {/* The plan's living document — side by side with the chat. */}
            {selectedConversationId ? (
              <div className="hidden min-w-0 basis-[44%] lg:flex">
                <PlanDoc
                  planId={selectedConversationId}
                  planTitle={conversations.find((c) => c.id === selectedConversationId)?.title ?? null}
                />
              </div>
            ) : null}
          </>
        ) : (
          <div className="grid h-full flex-1 place-items-center text-sm text-muted">
            {agentsLoading ? 'Loading the fleet…' : 'No agents available.'}
          </div>
        )}
      </main>

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

      {selectedConversationId && current && planOpen && (
        <PlanModal
          open={planOpen}
          onClose={() => setPlanOpen(false)}
          draftUrl={`/api/plan/${selectedConversationId}/draft`}
          agents={[current]}
        />
      )}
    </div>
  )
}
