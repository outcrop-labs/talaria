import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { cn } from '@/lib/cn'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { InlineCreate } from '@/components/ui/inline-create'
import { ChannelView } from '@/components/chat/channel-view'
import { ChannelSettingsModal } from '@/components/chat/channel-settings'
import { useAgents } from '@/lib/agents'
import { useSession } from '@/lib/session'
import { createChannel, useChannelDetail, useChannels } from '@/lib/channels'

export const Route = createFileRoute('/_app/channels')({
  component: ChannelsPage,
})

// Group chat — Slack-style channels where teammates and fleet agents talk in
// one place. Agents reply when @mentioned.
function ChannelsPage() {
  const qc = useQueryClient()
  const { data: session } = useSession()
  const { data: fleetData } = useAgents()
  const fleet = fleetData?.agents ?? []
  const { data: channels = [], isLoading } = useChannels()

  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)

  useEffect(() => {
    if (!selectedId && channels[0]) setSelectedId(channels[0].id)
    if (selectedId && !channels.some((c) => c.id === selectedId)) setSelectedId(channels[0]?.id ?? null)
  }, [channels, selectedId])

  const selected = channels.find((c) => c.id === selectedId) ?? null
  const { data: detail } = useChannelDetail(selectedId)

  const create = async (name: string) => {
    const c = await createChannel(name)
    await qc.invalidateQueries({ queryKey: ['channels'] })
    setSelectedId(c.id)
    setSettingsOpen(true) // straight into adding people/agents
  }

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex h-full w-60 shrink-0 flex-col border-r border-line-subtle bg-sidebar">
        <div className="border-b border-line-subtle p-3">
          <InlineCreate label="New channel" placeholder="channel name" onSubmit={(v) => void create(v)} className="w-full" />
        </div>
        <div className="flex-1 overflow-y-auto p-2">
          {channels.length === 0 ? (
            <div className="px-2 py-6 text-center text-xs text-muted">{isLoading ? 'Loading…' : 'No channels yet.'}</div>
          ) : (
            <ul className="space-y-0.5">
              {channels.map((c) => (
                <li key={c.id}>
                  <button
                    type="button"
                    onClick={() => setSelectedId(c.id)}
                    className={cn(
                      'w-full truncate rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
                      c.id === selectedId ? 'bg-card text-fg' : 'text-muted',
                    )}
                  >
                    <span className="mr-1 opacity-60">#</span>
                    {c.name}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </aside>

      <main className="min-h-0 min-w-0 flex-1">
        {selected ? (
          <div className="flex h-full min-h-0 flex-col">
            <header className="flex items-center gap-2 border-b border-line-subtle px-4 py-2.5">
              <span className="text-sm font-semibold text-fg">#{selected.name}</span>
              {selected.topic && <span className="truncate text-xs text-muted">{selected.topic}</span>}
              <span className="ml-auto" />
              {detail && (
                <span className="text-xs text-muted">
                  {detail.members.length} {detail.members.length === 1 ? 'person' : 'people'}
                  {detail.agents.length > 0 && ` · ${detail.agents.length} ${detail.agents.length === 1 ? 'agent' : 'agents'}`}
                </span>
              )}
              <Button variant="outline" size="sm" onClick={() => setSettingsOpen(true)}>
                Settings
              </Button>
            </header>
            <div className="min-h-0 flex-1">
              <ChannelView
                key={selected.id}
                channelId={selected.id}
                channelName={selected.name}
                channelAgents={detail?.agents ?? []}
                members={detail?.members ?? []}
                fleet={fleet}
              />
            </div>
          </div>
        ) : (
          <EmptyState
            icon="#"
            title="Group chat for people and agents"
            hint="Create a channel, add teammates and fleet agents, and @mention an agent to bring it into the conversation."
          />
        )}
      </main>

      {selected && detail && (
        <ChannelSettingsModal
          open={settingsOpen}
          onClose={() => setSettingsOpen(false)}
          channelId={selected.id}
          channelName={selected.name}
          detail={detail}
          fleet={fleet}
          selfUserId={session?.id ?? null}
          onDeleted={() => {
            setSettingsOpen(false)
            void qc.invalidateQueries({ queryKey: ['channels'] })
          }}
        />
      )}
    </div>
  )
}
