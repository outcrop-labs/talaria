import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useMemo, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { CheckCheck, ClipboardList, Plus, Settings, SquarePen } from 'lucide-react'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
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
//   DMs        teammates (channel machinery) and agents (nested threads that
//              distill into the activity brain and archive when idle)
// Talking to an agent starts a NEW thread by default — bounded context per
// topic, no giant-scrollback bloat riding along on every turn. Recent threads
// nest under the agent in the sidebar for resuming deliberately.
type Sel = { t: 'channel'; id: string } | { t: 'agent'; model: string; conversationId: string | null } | null

function CommsPage() {
  const qc = useQueryClient()
  const { data: session } = useSession()
  const { data: fleetData } = useAgents()
  const fleet = fleetData?.agents ?? []
  const { data: channels = [], isLoading } = useChannels()
  const { data: users = [] } = useUsers()
  const { data: conversations = [] } = useConversations('chat')

  const [sel, setSel] = useState<Sel>(null)
  // Bumped on every deliberate fresh-thread start; drives ChatView's reset.
  const [fresh, setFresh] = useState(0)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [planOpen, setPlanOpen] = useState(false)

  const newThread = (model: string) => {
    setSel({ t: 'agent', model, conversationId: null })
    setFresh((n) => n + 1)
  }

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
        <Section label="Channels" createPlaceholder="channel name" onCreate={(v) => void create(v, 'channel')}>
          {rooms.map((c) => (
            <RowButton key={c.id} active={sel?.t === 'channel' && sel.id === c.id} onClick={() => setSel({ t: 'channel', id: c.id })}>
              <span className="shrink-0 opacity-60">#</span>
              <span className="min-w-0 flex-1 truncate">{c.name}</span>
            </RowButton>
          ))}
          {rooms.length === 0 && <Hint>{isLoading ? 'Loading…' : 'Ambient, persistent talk.'}</Hint>}
        </Section>

        <Section label="Relays" createPlaceholder="what's it about?" onCreate={(v) => void create(v, 'group')}>
          {relays.map((c) => (
            <RowButton key={c.id} active={sel?.t === 'channel' && sel.id === c.id} onClick={() => setSel({ t: 'channel', id: c.id })}>
              <span className="shrink-0 opacity-60">⇄</span>
              <span className="min-w-0 flex-1 truncate">{c.name}</span>
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
                <Avatar name={u.name ?? u.email ?? '?'} className="h-5 w-5 shrink-0 text-[10px]" />
                <span className="min-w-0 flex-1 truncate">{u.name ?? u.email}</span>
              </RowButton>
            )
          })}
          {people.length === 0 && <Hint>Just you so far.</Hint>}
        </Section>

        <Section label="Agents">
          {fleet.map((a) => {
            const activeAgent = sel?.t === 'agent' && sel.model === a.id
            const threads = activeAgent ? conversations.filter((c) => c.agentModel === a.id).slice(0, 8) : []
            return (
              <li key={a.id}>
                <ul className="space-y-0.5">
                  {/* Clicking the agent = a fresh thread (bounded context by default). */}
                  <RowButton active={activeAgent && sel.conversationId === null} onClick={() => newThread(a.id)}>
                    <span className="shrink-0 opacity-60">◍</span>
                    <span className="min-w-0 flex-1 truncate">{a.label}</span>
                    {activeAgent && sel.conversationId === null && (
                      <span className="shrink-0 text-[10px] text-muted">new</span>
                    )}
                  </RowButton>
                  {threads.map((c) => (
                    <RowButton
                      key={c.id}
                      active={activeAgent && sel.conversationId === c.id}
                      onClick={() => setSel({ t: 'agent', model: a.id, conversationId: c.id })}
                      className="pl-7 text-xs"
                    >
                      <span className="min-w-0 flex-1 truncate">{c.title || 'Untitled'}</span>
                    </RowButton>
                  ))}
                </ul>
              </li>
            )
          })}
          {fleet.length === 0 && <Hint>No agents yet — hire on /agents.</Hint>}
        </Section>
      </aside>

      <main className="min-h-0 min-w-0 flex-1">
        {sel?.t === 'agent' ? (
          <AgentDmPane
            key={sel.model}
            model={sel.model}
            fleet={fleet}
            conversationId={sel.conversationId}
            newChatSignal={fresh}
            onNewThread={() => newThread(sel.model)}
            onCreated={(id) => {
              setSel({ t: 'agent', model: sel.model, conversationId: id })
              void qc.invalidateQueries({ queryKey: ['conversations'] })
            }}
          />
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
                <IconAction title="Draft tickets from this conversation" onClick={() => setPlanOpen(true)}>
                  <ClipboardList size={16} />
                </IconAction>
              )}
              {selected.kind === 'group' && (
                <IconAction title="Conclude — summarize what was decided, then archive" onClick={() => void conclude()}>
                  <CheckCheck size={16} />
                </IconAction>
              )}
              {selected.kind !== 'dm' && (
                <IconAction title="Settings — people, agents, rename" onClick={() => setSettingsOpen(true)}>
                  <Settings size={16} />
                </IconAction>
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

// One agent's DM pane. Threads are selected in the sidebar (nested under the
// agent); a fresh thread is the default — bounded context per topic.
function AgentDmPane({
  model,
  fleet,
  conversationId,
  newChatSignal,
  onNewThread,
  onCreated,
}: {
  model: string
  fleet: { id: string; label: string; tiers?: string[] }[]
  conversationId: string | null
  newChatSignal: number
  onNewThread: () => void
  onCreated: (id: string) => void
}) {
  const agent = fleet.find((a) => a.id === model)
  if (!agent) return null
  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex items-center gap-2 border-b border-line-subtle px-5 py-3">
        <span className="text-sm font-semibold text-fg">◍ {agent.label}</span>
        {conversationId === null && <span className="text-xs text-muted">new thread — history stays out of context</span>}
        <span className="ml-auto" />
        <IconAction title="New thread — fresh context" onClick={onNewThread}>
          <SquarePen size={16} />
        </IconAction>
      </header>
      <div className="min-h-0 flex-1">
        <ChatView
          agentModel={model}
          agentLabel={agent.label}
          tiers={agent.tiers ?? []}
          conversationId={conversationId}
          newChatSignal={newChatSignal}
          onCreated={onCreated}
        />
      </div>
    </div>
  )
}

// Sidebar section: the create affordance is a small "+" IN the heading (Slack-
// style) that expands to an inline name input — no chunky buttons under lists.
function Section({
  label,
  createPlaceholder,
  onCreate,
  children,
}: {
  label: string
  createPlaceholder?: string
  onCreate?: (name: string) => void
  children: React.ReactNode
}) {
  const [creating, setCreating] = useState(false)
  const [name, setName] = useState('')
  const submit = (cancelled: boolean) => {
    const v = name.trim()
    setCreating(false)
    setName('')
    if (v && !cancelled) onCreate?.(v)
  }
  return (
    <div className="mb-4">
      <div className="mb-1 flex items-center px-2">
        <span className="text-[11px] uppercase tracking-wide text-muted">{label}</span>
        {onCreate && (
          <button
            type="button"
            title={`New ${label.toLowerCase().replace(/s$/, '')}`}
            onClick={() => setCreating(true)}
            className="ml-auto grid h-5 w-5 place-items-center rounded text-muted transition-colors hover:bg-card hover:text-fg"
          >
            <Plus size={13} />
          </button>
        )}
      </div>
      {creating && (
        <div className="mb-1 px-1">
          <Input
            autoFocus
            size="sm"
            value={name}
            placeholder={createPlaceholder}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submit(false)
              else if (e.key === 'Escape') submit(true)
            }}
            onBlur={() => submit(false)}
          />
        </div>
      )}
      <ul className="space-y-0.5">{children}</ul>
    </div>
  )
}

function RowButton({
  active,
  onClick,
  className,
  children,
}: {
  active: boolean
  onClick: () => void
  className?: string
  children: React.ReactNode
}) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className={cn(
          'flex w-full min-w-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-card',
          active ? 'bg-card text-fg' : 'text-muted',
          className,
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

/** Compact header action: icon-only, tooltip via title. */
function IconAction({ title, onClick, children }: { title: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      className="grid h-7 w-7 shrink-0 place-items-center rounded-lg text-muted transition-colors hover:bg-card hover:text-fg"
    >
      {children}
    </button>
  )
}
