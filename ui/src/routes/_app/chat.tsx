import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ChatView } from '@/components/chat/chat-view'
import { ConversationSidebar } from '@/components/chat/conversation-sidebar'
import { useAgents } from '@/lib/agents'
import { useConversations, type Conversation } from '@/lib/conversations'

export const Route = createFileRoute('/_app/chat')({
  component: ChatPage,
})

function ChatPage() {
  const qc = useQueryClient()
  const { data: fleet, isLoading: agentsLoading } = useAgents()
  const agents = useMemo(() => fleet?.agents ?? [], [fleet])
  const { data: conversations = [] } = useConversations()

  const [selectedAgent, setSelectedAgent] = useState<string | null>(null)
  const [selectedConversationId, setSelectedConversationId] = useState<string | null>(null)
  const [newChatSignal, setNewChatSignal] = useState(0)

  useEffect(() => {
    if (!selectedAgent && agents[0]) setSelectedAgent(agents[0].id)
  }, [agents, selectedAgent])

  const selectConversation = (c: Conversation) => {
    setSelectedAgent(c.agentModel)
    setSelectedConversationId(c.id)
  }
  const selectAgent = (agentModel: string) => {
    setSelectedAgent(agentModel)
    setSelectedConversationId(null)
    setNewChatSignal((n) => n + 1)
  }
  const newChat = () => {
    if (selectedAgent) selectAgent(selectedAgent)
  }
  const onCreated = (id: string) => {
    setSelectedConversationId(id)
    void qc.invalidateQueries({ queryKey: ['conversations'] })
  }

  const current = agents.find((a) => a.id === selectedAgent)

  return (
    <div className="flex h-full min-h-0">
      <main className="min-h-0 flex-1">
        {selectedAgent && current ? (
          <ChatView
            key={selectedAgent}
            agentModel={selectedAgent}
            agentLabel={current.label}
            tiers={current.tiers ?? []}
            conversationId={selectedConversationId}
            newChatSignal={newChatSignal}
            onCreated={onCreated}
          />
        ) : (
          <div className="grid h-full place-items-center text-sm text-muted">
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
        onNewChat={newChat}
      />
    </div>
  )
}
