import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { EmptyState } from '@/components/ui/empty-state'
import { InlineCreate } from '@/components/ui/inline-create'
import { Select } from '@/components/ui/select'
import { alert, confirm } from '@/components/ui/confirm'
import { ChatView } from '@/components/chat/chat-view'
import { ChannelView } from '@/components/chat/channel-view'
import { ChannelSettingsModal } from '@/components/chat/channel-settings'
import { PlanModal } from '@/components/chat/plan-modal'
import { useAgents } from '@/lib/agents'
import { useSession } from '@/lib/session'
import { useUsers } from '@/lib/users'
import { useConversations } from '@/lib/conversations'
import { createChannel, openDm, useChannelDetail, useChannels, type Channel } from '@/lib/channels'

export const Route = createFileRoute('/_app/comms')({
  component: CommsPage,
})

// Comms — every conversation in one place, Slack-shaped but agent-native:
//   #channels  persistent, ambient (general talk, quick questions)
//   Relays     named ad-hoc gatherings of people + agents around a purpose;
//              they CONCLUDE (summary posted + indexed) and archive
//   DMs        teammates (channel machinery) and agents (durable chat threads
//              that distill into the activity brain and archive when idle)
type Sel = { t: 'channel'; id: string } | { t: 'agent'; model: string } | null

function CommsPage() {
  const qc = useQueryClient()
  const { data: session } = useSession()
  const { data: fleetData } = useAgents()
  const fleet = fleetData?.agents ?? []
  const { data: channels = [], isLoading } = useChannels()
  const { data: users = [] } = useUsers()

  const [sel, setSel] = useState<Sel>(null)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [planOpen, setPlanOpen] = useState(false)

  const rooms = channels.filter((c) => c.kind === 'channel')
  const relays = channels.filter((c) => c.kind === 'group')
  const dms = channels.filter((c) => c.kind === 'dm')
  const people = users.filter((u) => u.id !== session?.id)
  const dmByPeer = useMemo(() => new Map(dms.map((c) => [c.peer?.userId, c])), [dms])

  // Default to the first channel; heal a selection that vanished (archived).
  useEffect(() => {
    if (!sel && channels[0]) setSel({ t: 'channel', id: channels[0].id })
    if (sel?.t === 'channel' && !channels.some((c) => c.id === sel.id)) {
      setSel(channels[0] ? { t: 'channel', id: channels[0].id } : null)
    }
  }, [channels, sel])

  const selected: Channel | null = sel?.t === 'channel' ? (channels.find((c) => c.id === sel.id) ?? null) : null
  const { data: detail } = useChannelDetail(selected?.id ?? null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['channels'] })

  const create = async (name: string, kind: 'channel' | 'group') => {
    const c = await createChannel(name, kind)
    await refresh()
    setSel({ t: 'channel', id: c.id })
    setSettingsOpen(true) // straight into adding people/agents
  }

  const startDm = async (userId: string) => {
    const c = await openDm(userId)
    await refresh()
    setSel({ t: 'channel', id: c.id })
  }

  const conclude = async () => {
    if (!selected) return
    if (
      !(await confirm({
        title: 'Conclude relay',
        message: `Wrap up "${selected.name}"? A summary of what was decided is posted and indexed, then the relay archives.`,
        confirmLabel: 'Conclude',
      }))
    )
      return
    const r = await fetch(`/api/channels/${selected.id}/conclude`, { method: 'POST', credentials: 'same-origin' })
    const j = (await r.json().catch(() => ({}))) as { summary?: string; error?: string }
    if (!r.ok) return void alert({ title: 'Could not conclude', message: j.error ?? `failed (${r.status})` })
    await refresh()
    void alert({ title: `${selected.name} — concluded`, message: j.summary ?? 'Summarized and archived.' })
  }

  const peerLabel = (c: Channel) => c.peer?.name ?? c.peer?.email ?? 'teammate'
  const title = selected ? (selected.kind === 'dm' ? peerLabel(selected) : selected.name) : ''

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex h-full w-64 shrink-0 flex-col overflow-y-auto border-r border-line-subtle bg-sidebar p-3">
        <Section
          label="Channels"
          create={<InlineCreate label="New channel" placeholder="channel name" onSubmit={(v) => void create(v, 'channel')} className="w-full" />}
        >
          {rooms.map((c) => (
            <RowButton key={c.id} active={sel?.t === 'channel' && sel.id === c.id} onClick={() => setSel({ t: 'channel', id: c.id })}>
              <span className="mr-1 opacity-60">#</span>
              {c.name}
            </RowButton>
          ))}
          {rooms.length === 0 && <Hint>{isLoading ? 'Loading…' : 'Ambient, persistent talk.'}</Hint>}
        </Section>

        <Section
          label="Relays"
          create={<InlineCreate label="New relay" placeholder="what's it about?" onSubmit={(v) => void create(v, 'group')} className="w-full" />}
        >
          {relays.map((c) => (
            <RowButton key={c.id} active={sel?.t === 'channel' && sel.id === c.id} onClick={() => setSel({ t: 'channel', id: c.id })}>
              <span className="mr-1 opacity-60">⇄</span>
              {c.name}
            </RowButton>
          ))}
          {relays.length === 0 && <Hint>Gather people + agents around a purpose; conclude when done.</Hint>}
        </Section>

        <Section label="Teammates">
          {people.map((u) => {
            const dm = dmByPeer.get(u.id)
            return (
              <RowButton
                key={u.id}
                active={sel?.t === 'channel' && sel.id === dm?.id}
                onClick={() => (dm ? setSel({ t: 'channel', id: dm.id }) : void startDm(u.id))}
              >
                <Avatar name={u.name ?? u.email ?? '?'} className="mr-1.5 inline-flex h-4 w-4 align-text-bottom text-[9px]" />
                {u.name ?? u.email}
              </RowButton>
            )
          })}
          {people.length === 0 && <Hint>Just you so far.</Hint>}
        </Section>

        <Section label="Agents">
          {fleet.map((a) => (
            <RowButton key={a.id} active={sel?.t === 'agent' && sel.model === a.id} onClick={() => setSel({ t: 'agent', model: a.id })}>
              <span className="mr-1 opacity-60">◍</span>
              {a.label}
            </RowButton>
          ))}
          {fleet.length === 0 && <Hint>No agents yet — hire on /agents.</Hint>}
        </Section>
      </aside>

      <main className="min-h-0 min-w-0 flex-1">
        {sel?.t === 'agent' ? (
          <AgentDmPane key={sel.model} model={sel.model} fleet={fleet} />
        ) : selected ? (
          <div className="flex h-full min-h-0 flex-col">
            <header className="flex items-center gap-2 border-b border-line-subtle px-5 py-3">
              <span className="text-sm font-semibold text-fg">
                {selected.kind === 'channel' ? `#${title}` : selected.kind === 'group' ? `⇄ ${title}` : title}
              </span>
              {selected.topic && <span className="truncate text-xs text-muted">{selected.topic}</span>}
              <span className="ml-auto" />
              {detail && selected.kind !== 'dm' && (
                <span className="text-xs text-muted">
                  {detail.members.length} {detail.members.length === 1 ? 'person' : 'people'}
                  {detail.agents.length > 0 && ` · ${detail.agents.length} ${detail.agents.length === 1 ? 'agent' : 'agents'}`}
                </span>
              )}
              {(detail?.agents.length ?? 0) > 0 && (
                <Button variant="outline" size="sm" onClick={() => setPlanOpen(true)}>
                  Plan
                </Button>
              )}
              {selected.kind === 'group' && (
                <Button variant="outline" size="sm" onClick={() => void conclude()}>
                  Conclude
                </Button>
              )}
              {selected.kind !== 'dm' && (
                <Button variant="outline" size="sm" onClick={() => setSettingsOpen(true)}>
                  Settings
                </Button>
              )}
            </header>
            <div className="min-h-0 flex-1">
              <ChannelView
                key={selected.id}
                channelId={selected.id}
                channelName={title}
                channelAgents={detail?.agents ?? []}
                members={detail?.members ?? []}
                fleet={fleet}
              />
            </div>
          </div>
        ) : (
          <EmptyState
            icon="◈"
            title="All your conversations, one place"
            hint="Channels for ambient talk, relays for getting something decided with people and agents, DMs for everyone — human or agent."
          />
        )}
      </main>

      {selected && detail && planOpen && (
        <PlanModal
          open={planOpen}
          onClose={() => setPlanOpen(false)}
          draftUrl={`/api/channels/${selected.id}/plan`}
          agents={fleet.filter((a) => detail.agents.includes(a.id))}
        />
      )}
      {selected && detail && selected.kind !== 'dm' && (
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
            void refresh()
          }}
        />
      )}
    </div>
  )
}

// One agent's DM pane: the durable 1:1 thread, with a switcher over past
// conversations (until they distill + archive) and a fresh-thread action.
function AgentDmPane({ model, fleet }: { model: string; fleet: { id: string; label: string; tiers?: string[] }[] }) {
  const qc = useQueryClient()
  const { data: conversations = [] } = useConversations('chat')
  const mine = conversations.filter((c) => c.agentModel === model)
  const [conversationId, setConversationId] = useState<string | null>(null)
  const [newChatSignal, setNewChatSignal] = useState(0)
  const agent = fleet.find((a) => a.id === model)

  // Land on the latest thread with this agent (if any).
  useEffect(() => {
    if (conversationId === null && mine[0]) setConversationId(mine[0].id)
  }, [mine, conversationId])

  if (!agent) return null
  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center gap-2 border-b border-line-subtle px-5 py-3">
        <span className="text-sm font-semibold text-fg">◍ {agent.label}</span>
        <span className="ml-auto" />
        {mine.length > 0 && (
          <Select
            size="sm"
            value={conversationId ?? ''}
            onChange={(e) => setConversationId(e.target.value || null)}
            className="max-w-56"
          >
            {mine.map((c) => (
              <option key={c.id} value={c.id}>
                {c.title || 'Untitled'}
              </option>
            ))}
          </Select>
        )}
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            setConversationId(null)
            setNewChatSignal((n) => n + 1)
          }}
        >
          New thread
        </Button>
      </header>
      <div className="min-h-0 flex-1">
        <ChatView
          agentModel={model}
          agentLabel={agent.label}
          tiers={agent.tiers ?? []}
          conversationId={conversationId}
          newChatSignal={newChatSignal}
          onCreated={(id) => {
            setConversationId(id)
            void qc.invalidateQueries({ queryKey: ['conversations'] })
          }}
        />
      </div>
    </div>
  )
}

function Section({ label, create, children }: { label: string; create?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="mb-4">
      <div className="mb-1.5 px-2 text-[11px] uppercase tracking-wide text-muted">{label}</div>
      <ul className="space-y-0.5">{children}</ul>
      {create && <div className="mt-1.5">{create}</div>}
    </div>
  )
}

function RowButton({ active, onClick, children }: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className={cn(
          'w-full truncate rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
          active ? 'bg-card text-fg' : 'text-muted',
        )}
      >
        {children}
      </button>
    </li>
  )
}

const Hint = ({ children }: { children: React.ReactNode }) => (
  <li className="px-2 py-1 text-[11px] leading-relaxed text-muted">{children}</li>
)
