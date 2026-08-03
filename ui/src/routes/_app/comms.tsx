import { createFileRoute } from '@tanstack/react-router'
import { Skeleton, SkeletonRows } from '@/components/ui/skeleton'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { CheckCheck, ClipboardList, Plus, Settings, SquarePen } from 'lucide-react'
import { cn } from '@/lib/cn'
import { Avatar } from '@/components/ui/avatar'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { GeneratingOverlay } from '@/components/ui/generating'
import { alert, confirm, prompt } from '@/components/ui/confirm'
import { ChatView } from '@/components/chat/chat-view'
import { copyAppLink, useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu'
import { CountPill, Rail, RailRow, RailSurface } from '@/components/app/surface'
import { QueryError } from '@/components/ui/query-state'
import { ChannelView } from '@/components/chat/channel-view'
import { ChannelSettingsModal } from '@/components/chat/channel-settings'
import { PlanModal } from '@/components/chat/plan-modal'
import { useAgents } from '@/lib/agents'
import { useSession, useHasPerm } from '@/lib/session'
import { useUsers } from '@/lib/users'
import { useConversations } from '@/lib/conversations'
import {
  addChannelAgent,
  addChannelMember,
  createChannel,
  markChannelRead,
  openDm,
  removeChannelAgent,
  removeChannelMember,
  useChannelDetail,
  useChannels,
  type Channel,
} from '@/lib/channels'

export const Route = createFileRoute('/_app/comms')({
  component: CommsPage,
  // ?c=<channelId> deep-links a channel/DM; ?a=<agentModel>&x=<convId>
  // deep-links an agent conversation (agent-outreach notifications land here).
  validateSearch: (search: Record<string, unknown>): { c?: string; a?: string; x?: string } => ({
    ...(typeof search.c === 'string' && search.c ? { c: search.c } : {}),
    ...(typeof search.a === 'string' && search.a ? { a: search.a } : {}),
    ...(typeof search.x === 'string' && search.x ? { x: search.x } : {}),
  }),
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
  // Every rail section below keeps its whole query, not just `data` — a
  // destructured `= []` throws the rejection away and the section then renders
  // its friendly "you have none yet" line over a 500. Four separate reads feed
  // this rail, so each owns its own failure marker (see `RailFailure`).
  const fleetQuery = useAgents()
  const { data: fleetData, isLoading: fleetLoading } = fleetQuery
  const fleet = fleetData?.agents ?? []
  const channelsQuery = useChannels()
  const { data: channels = [], isLoading } = channelsQuery
  const usersQuery = useUsers()
  const { data: users = [], isLoading: usersLoading } = usersQuery
  const conversationsQuery = useConversations('chat')
  const { data: conversations = [], isLoading: conversationsLoading } = conversationsQuery

  // The URL IS the selection: ?c=<channel> or ?a=<agent>&x=<thread>. Every
  // pick navigates (push), so back/forward walks your reading order and any
  // view is copy-linkable.
  const search = Route.useSearch()
  const navigate = Route.useNavigate()
  const sel: Sel = search.c
    ? { t: 'channel', id: search.c }
    : search.a
      ? { t: 'agent', model: search.a, conversationId: search.x ?? null }
      : null
  const setSel = (next: Sel, opts: { replace?: boolean } = {}) =>
    void navigate({
      search:
        next?.t === 'channel'
          ? { c: next.id }
          : next?.t === 'agent'
            ? { a: next.model, ...(next.conversationId ? { x: next.conversationId } : {}) }
            : {},
      replace: opts.replace,
    })
  // Agents whose thread list is pinned open (chevron) without being selected.
  const [expandedAgents, setExpandedAgents] = useState<Set<string>>(new Set())
  // Bumped on every deliberate fresh-thread start; drives ChatView's reset.
  const [fresh, setFresh] = useState(0)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [planOpen, setPlanOpen] = useState(false)

  const newThread = (model: string) => {
    setSel({ t: 'agent', model, conversationId: null })
    setFresh((n) => n + 1)
  }

  // Clicking an agent lands on its WORKING thread when a reply is in flight —
  // leaving mid-stream and coming back must show the agent still at it, not a
  // blank new thread that makes the work look lost. Otherwise: fresh thread.
  const openAgent = (model: string) => {
    const working = conversations.find((c) => c.agentModel === model && c.working)
    if (working) setSel({ t: 'agent', model, conversationId: working.id })
    else newThread(model)
  }

  // Clicking an agent BEFORE the conversations query resolves can't see its
  // working thread — when the data first lands, upgrade a still-fresh agent
  // selection to that thread so in-flight work is resumed, not shadowed.
  // Once only (on the load transition): deliberate new threads afterwards
  // must never be yanked into the working thread.
  const upgradedOnLoad = useRef(false)
  useEffect(() => {
    if (conversationsLoading || upgradedOnLoad.current) return
    upgradedOnLoad.current = true
    if (sel?.t !== 'agent' || sel.conversationId !== null) return
    const working = conversations.find((c) => c.agentModel === sel.model && c.working)
    if (working) setSel({ t: 'agent', model: sel.model, conversationId: working.id }, { replace: true })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conversationsLoading, conversations])

  const rooms = channels.filter((c) => c.kind === 'channel')
  const relays = channels.filter((c) => c.kind === 'group')
  const dms = channels.filter((c) => c.kind === 'dm')
  const people = users.filter((u) => u.id !== session?.id)
  const dmByPeer = useMemo(() => new Map(dms.map((c) => [c.peer?.userId, c])), [dms])

  // Default to the first channel; heal a selection that vanished (archived).
  // Both are replace-navigations — housekeeping shouldn't pollute history.
  useEffect(() => {
    if (channels.length === 0 && isLoading) return
    if (!sel && channels[0]) setSel({ t: 'channel', id: channels[0].id }, { replace: true })
    if (sel?.t === 'channel' && channels.length > 0 && !channels.some((c) => c.id === sel.id)) {
      setSel(channels[0] ? { t: 'channel', id: channels[0].id } : null, { replace: true })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [channels, sel, isLoading])

  const selected: Channel | null = sel?.t === 'channel' ? (channels.find((c) => c.id === sel.id) ?? null) : null
  const { data: detail, isLoading: detailLoading } = useChannelDetail(selected?.id ?? null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['channels'] })

  // Right-click menus on sidebar rows — shortcuts to actions the rows already
  // perform (open/copy the URL-driven selection, advance the read cursor).
  const { openMenu, menu } = useContextMenu()

  // Sidebar "Mark read": the cursor advances to the channel's latest seq, and
  // only the messages list knows it — fetch it, post the cursor (the same call
  // ChannelView makes on open), then refresh the badges. Best-effort.
  const markRead = async (id: string) => {
    try {
      const r = await fetch(`/api/channels/${id}/messages`, { credentials: 'same-origin' })
      const { messages = [] } = ((await r.json()) ?? {}) as { messages?: { seq: number }[] }
      const latest = messages[messages.length - 1]?.seq ?? 0
      if (!latest) return
      await markChannelRead(id, latest)
      await qc.invalidateQueries({ queryKey: ['channels'] })
    } catch {
      // badges are advisory; a failed mark-read fixes itself on open
    }
  }

  const channelRowMenu = (c: Channel): ContextMenuEntry[] => [
    { label: 'Open', onSelect: () => setSel({ t: 'channel', id: c.id }) },
    { label: 'Copy link', onSelect: () => copyAppLink(`/comms?c=${c.id}`) },
    { label: 'Mark read', disabled: !c.unreadCount, onSelect: () => void markRead(c.id) },
  ]

  const mayCreateChannels = useHasPerm('comms.channels')
  const mayStartRelays = useHasPerm('comms.relays')
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

  const [concluding, setConcluding] = useState(false)
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
    setConcluding(true)
    const r = await fetch(`/api/channels/${selected.id}/conclude`, { method: 'POST', credentials: 'same-origin' }).finally(() =>
      setConcluding(false),
    )
    const j = (await r.json().catch(() => ({}))) as { summary?: string; error?: string }
    if (!r.ok) return void alert({ title: 'Could not conclude', message: j.error ?? `failed (${r.status})` })
    await refresh()
    void alert({ title: `${selected.name} concluded`, message: j.summary ?? 'Summarized and archived.' })
  }

  const peerLabel = (c: Channel) => c.peer?.name ?? c.peer?.email ?? 'teammate'
  const title = selected ? (selected.kind === 'dm' ? peerLabel(selected) : selected.name) : ''

  return (
    <RailSurface>
      <Rail title="Comms">
        <Section label="Channels" createPlaceholder="channel name" onCreate={mayCreateChannels ? (v) => void create(v, 'channel') : undefined}>
          {rooms.map((c) => (
            <RailRow key={c.id} active={sel?.t === 'channel' && sel.id === c.id} onClick={() => setSel({ t: 'channel', id: c.id })}>
              {/* display:contents wrapper — carries the context menu without touching row layout */}
              <span className="contents" onContextMenu={(e) => openMenu(e, channelRowMenu(c))}>
                <span className="shrink-0 opacity-60">#</span>
                <span className="min-w-0 flex-1 truncate">{c.name}</span>
                <CountPill count={c.unreadCount} />
              </span>
            </RailRow>
          ))}
          {rooms.length === 0 &&
            (isLoading ? (
              <SkeletonRows rows={4} className="px-2 py-1.5" />
            ) : channelsQuery.isError && channelsQuery.data === undefined ? (
              <RailFailure
                error={channelsQuery.error}
                title="Could not load channels"
                onRetry={() => void channelsQuery.refetch()}
              />
            ) : (
              <Hint>Ambient, persistent talk.</Hint>
            ))}
        </Section>

        <Section label="Relays" createPlaceholder="what's it about?" onCreate={mayStartRelays ? (v) => void create(v, 'group') : undefined}>
          {relays.map((c) => (
            <RailRow key={c.id} active={sel?.t === 'channel' && sel.id === c.id} onClick={() => setSel({ t: 'channel', id: c.id })}>
              <span className="contents" onContextMenu={(e) => openMenu(e, channelRowMenu(c))}>
                <span className="shrink-0 opacity-60">⇄</span>
                <span className="min-w-0 flex-1 truncate">{c.name}</span>
                <CountPill count={c.unreadCount} />
              </span>
            </RailRow>
          ))}
          {relays.length === 0 &&
            (isLoading ? (
              <SkeletonRows rows={3} className="px-2 py-1.5" />
            ) : channelsQuery.isError && channelsQuery.data === undefined ? (
              <RailFailure
                error={channelsQuery.error}
                title="Could not load relays"
                onRetry={() => void channelsQuery.refetch()}
              />
            ) : (
              <Hint>Gather people + agents around a purpose; conclude when done.</Hint>
            ))}
        </Section>

        <Section label="Teammates">
          {people.map((u) => {
            const dm = dmByPeer.get(u.id)
            return (
              <RailRow
                key={u.id}
                active={sel?.t === 'channel' && sel.id === dm?.id}
                onClick={() => (dm ? setSel({ t: 'channel', id: dm.id }) : void startDm(u.id))}
              >
                <span
                  className="contents"
                  onContextMenu={(e) =>
                    openMenu(e, dm ? channelRowMenu(dm) : [{ label: 'Open', onSelect: () => void startDm(u.id) }])
                  }
                >
                  <Avatar name={u.name ?? u.email ?? '?'} className="h-5 w-5 shrink-0 text-[10px]" />
                  <span className="min-w-0 flex-1 truncate">{u.name ?? u.email}</span>
                  <CountPill count={dm?.unreadCount} />
                </span>
              </RailRow>
            )
          })}
          {people.length === 0 &&
            (usersLoading ? (
              <SkeletonRows rows={4} avatar className="px-2 py-1.5" />
            ) : usersQuery.isError && usersQuery.data === undefined ? (
              // "Just you so far." over a failed directory read is how a
              // 20-person org gets told it is one person.
              <RailFailure
                error={usersQuery.error}
                title="Could not load teammates"
                onRetry={() => void usersQuery.refetch()}
              />
            ) : (
              <Hint>Just you so far.</Hint>
            ))}
        </Section>

        <Section label="Agents">
          {fleet.map((a) => {
            const activeAgent = sel?.t === 'agent' && sel.model === a.id
            const agentThreads = conversations.filter((c) => c.agentModel === a.id)
            // Threads unfold for the active agent, or via the chevron —
            // peeking at an agent's threads shouldn't require selecting it.
            const expanded = activeAgent || expandedAgents.has(a.id)
            const threads = expanded ? agentThreads.slice(0, 8) : []
            return (
              <li key={a.id}>
                <ul className="space-y-0.5">
                  {/* Clicking the agent = its working thread if one is live, else fresh. */}
                  <RailRow active={activeAgent && sel.conversationId === null} onClick={() => openAgent(a.id)}>
                    <span
                      className="contents"
                      onContextMenu={(e) => openMenu(e, [{ label: 'New thread', onSelect: () => newThread(a.id) }])}
                    >
                      <span className="shrink-0 opacity-60">◍</span>
                      <span className="min-w-0 flex-1 truncate">{a.label}</span>
                      {conversations.some((c) => c.agentModel === a.id && c.working) && (
                        <span className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-accent" title="working on a reply" />
                      )}
                      {activeAgent && sel.conversationId === null && (
                        <span className="shrink-0 text-[10px] text-muted">new</span>
                      )}
                      {agentThreads.length > 0 && !activeAgent && (
                        <span
                          role="button"
                          title={expanded ? 'Hide threads' : `Show threads (${agentThreads.length})`}
                          className="shrink-0 rounded px-0.5 text-[10px] text-muted hover:text-fg"
                          onClick={(e) => {
                            e.stopPropagation()
                            setExpandedAgents((prev) => {
                              const next = new Set(prev)
                              if (next.has(a.id)) next.delete(a.id)
                              else next.add(a.id)
                              return next
                            })
                          }}
                        >
                          {expanded ? '▾' : '▸'}
                        </span>
                      )}
                    </span>
                  </RailRow>
                  {threads.map((c) => (
                    <RailRow
                      key={c.id}
                      active={activeAgent && sel.conversationId === c.id}
                      onClick={() => setSel({ t: 'agent', model: a.id, conversationId: c.id })}
                      className="pl-7 text-xs"
                    >
                      <span
                        className="contents"
                        onContextMenu={(e) =>
                          openMenu(e, [
                            { label: 'Open', onSelect: () => setSel({ t: 'agent', model: a.id, conversationId: c.id }) },
                            { label: 'Copy link', onSelect: () => copyAppLink(`/comms?a=${a.id}&x=${c.id}`) },
                            {
                              label: 'Rename',
                              onSelect: () => {
                                void prompt({ title: 'Rename thread', defaultValue: c.title ?? '', placeholder: 'Thread name', confirmLabel: 'Rename' }).then(async (name) => {
                                  if (!name?.trim()) return
                                  await fetch(`/api/conversations/${c.id}`, {
                                    method: 'PATCH',
                                    credentials: 'same-origin',
                                    headers: { 'content-type': 'application/json' },
                                    body: JSON.stringify({ title: name.trim() }),
                                  })
                                  void qc.invalidateQueries({ queryKey: ['conversations'] })
                                })
                              },
                            },
                          ])
                        }
                      >
                        <span className="min-w-0 flex-1 truncate">{c.title || 'Untitled'}</span>
                        {c.working && <span className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-accent" />}
                      </span>
                    </RailRow>
                  ))}
                </ul>
              </li>
            )
          })}
          {fleet.length === 0 &&
            (fleetLoading ? (
              <SkeletonRows rows={3} avatar className="px-2 py-1.5" />
            ) : fleetQuery.isError && fleetData === undefined ? (
              <RailFailure
                error={fleetQuery.error}
                title="Could not load your agents"
                onRetry={() => void fleetQuery.refetch()}
              />
            ) : (
              <Hint>No agents yet. Hire on /agents.</Hint>
            ))}
          {/* Threads nest under the agent rows above, so a failed conversation
              read makes every agent look like it has never been talked to.
              The rows stay (good fleet data is not thrown away) — the marker
              says the threads are missing rather than absent. */}
          {conversationsQuery.isError && conversationsQuery.data === undefined && (
            <RailFailure
              error={conversationsQuery.error}
              title="Could not load your threads"
              onRetry={() => void conversationsQuery.refetch()}
            />
          )}
        </Section>
      </Rail>

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
            <header className="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
              <span className="text-sm font-semibold text-fg">
                {selected.kind === 'channel' ? `#${title}` : selected.kind === 'group' ? `⇄ ${title}` : title}
              </span>
              {selected.topic && <span className="truncate text-xs text-muted">{selected.topic}</span>}
              <span className="ml-auto" />
              {/* Pill-shaped placeholders while membership loads — the header
                  holds its shape instead of the pickers popping in. */}
              {!detail && detailLoading && selected.kind !== 'dm' && (
                <>
                  <Skeleton className="h-6 w-20 rounded-full" />
                  <Skeleton className="h-6 w-20 rounded-full" delay={0.12} />
                </>
              )}
              {detail && selected.kind !== 'dm' && (
                <>
                  {/* Membership managed right here — the settings modal is for renames/danger. */}
                  <HeaderPicker
                    label={`${detail.members.length} ${detail.members.length === 1 ? 'person' : 'people'}`}
                    options={users
                      .filter((u) => u.email)
                      .map((u) => ({
                        value: u.id,
                        label: u.name ?? u.email ?? u.id,
                        locked: detail.members.some((m) => m.userId === u.id && m.role === 'owner'),
                      }))}
                    selected={detail.members.map((m) => m.userId)}
                    onToggle={async (id, on) => {
                      const email = users.find((u) => u.id === id)?.email
                      if (on && email) await addChannelMember(selected.id, email)
                      if (!on) await removeChannelMember(selected.id, id)
                      await qc.invalidateQueries({ queryKey: ['channel', selected.id] })
                    }}
                  />
                  <HeaderPicker
                    label={`${detail.agents.length} ${detail.agents.length === 1 ? 'agent' : 'agents'}`}
                    options={fleet.map((a) => ({ value: a.id, label: a.label }))}
                    selected={detail.agents}
                    onToggle={async (model, on) => {
                      if (on) await addChannelAgent(selected.id, model)
                      else await removeChannelAgent(selected.id, model)
                      await qc.invalidateQueries({ queryKey: ['channel', selected.id] })
                    }}
                  />
                </>
              )}
              {(detail?.agents.length ?? 0) > 0 && (
                <IconAction title="Draft tickets from this conversation" onClick={() => setPlanOpen(true)}>
                  <ClipboardList size={16} />
                </IconAction>
              )}
              {selected.kind === 'group' && (
                <IconAction title="Conclude: summarize what was decided, then archive" onClick={() => void conclude()}>
                  <CheckCheck size={16} />
                </IconAction>
              )}
              {selected.kind !== 'dm' && (
                <IconAction title="Settings: people, agents, rename" onClick={() => setSettingsOpen(true)}>
                  <Settings size={16} />
                </IconAction>
              )}
            </header>
            <div className="relative min-h-0 flex-1">
              {concluding && <GeneratingOverlay label="Concluding: summarizing what was decided, then archiving" />}
              {/* ChannelView fetches its own channel detail (react-query
                  dedupes with the header's call) so the message pane renders
                  independently of this fetch. */}
              <ChannelView key={selected.id} channelId={selected.id} channelName={title} fleet={fleet} />
            </div>
          </div>
        ) : (
          <EmptyState
            icon="◈"
            title="All your conversations, one place"
            hint="Channels for ambient talk, relays for getting something decided with people and agents, DMs for everyone, human or agent."
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
      {menu}
    </RailSurface>
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
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
        <span className="text-sm font-semibold text-fg">◍ {agent.label}</span>
        {conversationId === null && <span className="text-xs text-muted">new thread; history stays out of context</span>}
        <span className="ml-auto" />
        <IconAction title="New thread: fresh context" onClick={onNewThread}>
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
        <span className="text-[10px] font-semibold uppercase tracking-wide text-muted">{label}</span>
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

const Hint = ({ children }: { children: React.ReactNode }) => (
  <li className="px-2 py-1 text-[11px] leading-relaxed text-muted">{children}</li>
)

// A rail section whose read FAILED. It sits exactly where the list would have
// been, so a broken section can never be mistaken for an empty one — "No agents
// yet. Hire on /agents." over a 500 tells an owner to re-hire a fleet that is
// still there. `<li>` because Section renders its children inside a `<ul>`.
const RailFailure = ({ error, title, onRetry }: { error: unknown; title: string; onRetry: () => void }) => (
  <li className="px-2 py-1.5">
    <QueryError variant="inline" error={error} title={title} onRetry={onRetry} />
  </li>
)

// A neat little header multiselect: a pill ("3 people ▾") opening a checklist
// popover. Toggles apply immediately; locked rows (owners) can't be removed.
function HeaderPicker({
  label,
  options,
  selected,
  onToggle,
}: {
  label: string
  options: { value: string; label: string; locked?: boolean }[]
  selected: string[]
  onToggle: (value: string, on: boolean) => Promise<void>
}) {
  const [open, setOpen] = useState(false)
  const picked = new Set(selected)
  const chosen = options.filter((o) => picked.has(o.value))
  return (
    <div className="relative shrink-0">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={cn(
          'flex items-center gap-1.5 rounded-lg border border-line-subtle px-2 py-1 text-xs transition-colors hover:text-fg',
          open ? 'bg-card text-fg' : 'text-muted',
        )}
      >
        {/* Truncated avatar stack — fixed width so the header never jiggles. */}
        <span className="flex w-11 shrink-0 justify-start -space-x-2">
          {chosen.slice(0, 3).map((o) => (
            <Avatar key={o.value} name={o.label} className="h-5 w-5 shrink-0 text-[9px] ring-2 ring-[color:var(--theme-bg)]" />
          ))}
          {chosen.length > 3 && (
            <span className="grid h-5 w-5 shrink-0 place-items-center rounded-full bg-card text-[9px] text-muted ring-2 ring-[color:var(--theme-bg)]">
              +{chosen.length - 3}
            </span>
          )}
          {chosen.length === 0 && (
            <span className="grid h-5 w-5 shrink-0 place-items-center rounded-full border border-dashed border-line-subtle text-[9px] text-muted">
              +
            </span>
          )}
        </span>
        {label} <span className="opacity-60">▾</span>
      </button>
      {open && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
          <div className="mercury-panel absolute right-0 top-full z-20 mt-1 max-h-64 w-56 overflow-y-auto rounded-xl p-1">
            {options.length === 0 && <div className="px-2 py-1.5 text-xs text-muted">Nothing to add.</div>}
            {options.map((o) => {
              const on = picked.has(o.value)
              return (
                <button
                  key={o.value}
                  type="button"
                  disabled={o.locked}
                  title={o.locked ? 'The owner stays' : undefined}
                  onClick={() => void onToggle(o.value, !on)}
                  className={cn(
                    'flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-sm transition-colors',
                    o.locked ? 'cursor-default opacity-60' : 'hover:bg-card',
                    on ? 'text-fg' : 'text-muted',
                  )}
                >
                  <Avatar name={o.label} className="h-5 w-5 shrink-0 text-[10px]" />
                  <span className="min-w-0 flex-1 truncate">{o.label}</span>
                  {on && <span className="shrink-0 text-[color:var(--theme-success)]">✓</span>}
                </button>
              )
            })}
          </div>
        </>
      )}
    </div>
  )
}

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
