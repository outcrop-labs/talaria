<script lang="ts">
  import { pathId } from '@/lib/route-tabs'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigate, route } from '@/router'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import ChatView from '@/components/chat/ChatView.svelte'
  import ConversationSidebar from '@/components/chat/ConversationSidebar.svelte'
  import type { SidebarFailure } from '@/components/chat/conversation-sidebar'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import { planRowMenu } from '@/components/record-menus'
  import RailSurface from '@/components/app/RailSurface.svelte'
  import Stage from '@/components/app/Stage.svelte'
  import StageHeader from '@/components/app/StageHeader.svelte'
  import ComposerPicker from '@/components/chat/ComposerPicker.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { useHasPerm } from '@/lib/session'
  import { useAgents } from '@/lib/agents'
  import { useStickyAgent } from '@/lib/sticky-agent.svelte'
  import NoModelBump from '@/components/setup/NoModelBump.svelte'
  import {
    archiveConversation,
    deleteConversation,
    renameConversation,
    restoreConversation,
    useConversations,
    type Conversation,
  } from '@/lib/conversations.svelte'

  // Work surface: an agentic work session on the LEFT (the stream, its live
  // tool calls, and — from W4 — the approval cards the agent's writes queue
  // behind), a document on the RIGHT. A session is a conversation with
  // kind='work', which is why it inherits the Plan idiom whole: owner and
  // collaborators, per-turn author labels, presence, the read cursor.
  //
  // WHAT IS NOT HERE YET (this is W1 — the skeleton):
  //   W2  sharing + presence in the header (the Plan idiom's own controls)
  //   W3  the document pane — native artifacts first, then the editable
  //       Google embed. The frame below is already two-pane so that landing
  //       it is a fill-in and not a re-layout.
  //   W4  approval cards inline in the stream
  // Nothing here should have to move for those; they land inside the shapes
  // this file already draws.

  const qc = useQueryClient()
  const mayManageAgents = useHasPerm('agents.manage')
  // Both reads keep their query object: the rail and the stage each render a
  // sentence ("No sessions yet with this agent.", "No agents available.")
  // that is only true of a request that SUCCEEDED and came back empty.
  const fleetQuery = useAgents()
  const agentsLoading = $derived(fleetQuery.isLoading)
  const agents = $derived(fleetQuery.data?.agents ?? [])
  const conversationsQuery = useConversations('work')
  const conversations = $derived(conversationsQuery.data ?? [])
  const archivedQuery = useConversations('work', true)
  const archived = $derived(archivedQuery.data ?? [])
  const conversationsLoading = $derived(conversationsQuery.isLoading)
  // Stale beats blank: a failed BACKGROUND refetch still has good data to show,
  // so only a failure with nothing behind it becomes a visible failure.
  const agentsFailure: SidebarFailure = $derived(
    fleetQuery.isError && fleetQuery.data === undefined
      ? { error: fleetQuery.error, retry: () => void fleetQuery.refetch() }
      : null,
  )
  const conversationsFailure: SidebarFailure = $derived(
    conversationsQuery.isError && conversationsQuery.data === undefined
      ? { error: conversationsQuery.error, retry: () => void conversationsQuery.refetch() }
      : null,
  )
  const archivedFailure: SidebarFailure = $derived(
    archivedQuery.isError && archivedQuery.data === undefined
      ? { error: archivedQuery.error, retry: () => void archivedQuery.refetch() }
      : null,
  )
  const sticky = useStickyAgent('work', () => agents)
  const selectedAgent = $derived(sticky.selected)
  const pickAgent = sticky.select
  // THE URL IS THE SESSION SELECTION (/work/<id>) — linkable, back/forward-able.
  const selectedSessionId = $derived(pathId(route.pathname, '/work'))
  const setSelectedSessionId = (id: string | null, opts: { replace?: boolean } = {}) => {
    if (id) void navigate('/work/:sessionId', { params: { sessionId: id }, replace: opts.replace })
    else void navigate('/work', { replace: opts.replace })
  }
  let newChatSignal = $state(0)
  // The model-tier pick lives in the VIEW, not the composer: a session's chat
  // surface is attach + text + submit only, and the sidebar owns the agent.
  // A new session starts on the agent's main model ('').
  let sessionTier = $state('')

  const selectConversation = (c: Conversation) => {
    pickAgent(c.agentModel)
    setSelectedSessionId(c.id)
  }
  const selectAgent = (agentModel: string) => {
    pickAgent(agentModel)
    setSelectedSessionId(null)
    newChatSignal += 1
    sessionTier = ''
  }
  const newSession = () => {
    if (selectedAgent) selectAgent(selectedAgent)
  }
  const onCreated = (id: string) => {
    setSelectedSessionId(id)
    void qc.invalidateQueries({ queryKey: ['conversations'] })
  }

  // A deep link may reference a session whose AGENT isn't the sticky pick —
  // align the agent once the list resolves (the URL itself stays put).
  $effect(() => {
    if (!selectedSessionId) return
    const target = [...conversations, ...archived].find((c) => c.id === selectedSessionId)
    if (target && target.agentModel !== sticky.selected) pickAgent(target.agentModel)
  })

  const current = $derived(agents.find((a) => a.id === selectedAgent))

  // The tier chip's shape. This view lifts the pick out of its (minimal)
  // composer, so it spells out what the chip renders: '' is the agent's main
  // model and the chip's bottom rung — 'main' on the chip, 'main model' in the
  // row it stands on.
  const tierNames = $derived(current?.tiers ?? [])
  const tierOptions = $derived([
    { value: '', label: 'main model' },
    ...tierNames.map((name) => ({ value: name, label: name })),
  ])
  const tierMeter = $derived({
    total: tierOptions.length,
    lit: Math.max(0, tierOptions.findIndex((o) => o.value === sessionTier)) + 1,
  })

  const selected = $derived(
    conversations.find((c) => c.id === selectedSessionId) ??
      archived.find((c) => c.id === selectedSessionId) ??
      null,
  )
  const selectedIsArchived = $derived.by(() => {
    const c = selected
    return c != null && !conversations.some((row) => row.id === c.id)
  })
  const menu = useContextMenu()
  const quiet =
    'flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg'
  const refreshSessions = () => void qc.invalidateQueries({ queryKey: ['conversations'] })
  const renameSession = (c: Conversation) => {
    void renameConversation(c.id, c.title, 'session').then((ok) => {
      if (ok) refreshSessions()
    })
  }
  const archiveSession = (c: Conversation) => {
    void archiveConversation(c.id).then((ok) => {
      if (!ok) return
      refreshSessions()
      if (c.id === selectedSessionId) setSelectedSessionId(null)
    })
  }
  const restoreSession = (c: Conversation) => {
    void restoreConversation(c.id).then((ok) => {
      if (ok) refreshSessions()
    })
  }
  const deleteSession = (c: Conversation) => {
    void deleteConversation(c.id, c.title, 'session').then((ok) => {
      if (!ok) return
      refreshSessions()
      if (c.id === selectedSessionId) setSelectedSessionId(null)
    })
  }
  const openSessionMenu = (e: MouseEvent, c: Conversation, archivedRow: boolean) => {
    menu.openMenu(
      e,
      planRowMenu(c, {
        path: `/work/${c.id}`,
        open: () => selectConversation(c),
        archived: archivedRow,
        onRename: () => renameSession(c),
        onArchive: () => archiveSession(c),
        onRestore: () => restoreSession(c),
        onDelete: () => deleteSession(c),
      }),
    )
  }

  // The selected session names the view. Effect-claimed like Plan, Research
  // and Boards — the list query lands after mount, and the pathname key keeps
  // the fallback honest on deep links. An unsaved "New session" claims
  // nothing: it isn't a place yet.
  //
  // The claim is INERT right now: the dock-era top strip dropped its title row
  // (TopStrip's own doctrine), so nothing reads `viewTitleClaim` today. It
  // stays because the three sibling views all make it, and the day a surface
  // wants view titles again this one should not be the view that forgot.
  $effect(() => {
    if (!selected) return
    const name = selected.title || 'Untitled session'
    claimViewTitle(name, { trail: [name] })
  })
</script>

{#snippet hireAction()}
  <Button size="sm" onclick={() => void navigate('/agents')}>Hire an agent</Button>
{/snippet}

{#snippet headerActions()}
  <div class="flex items-center gap-3">
    {#if selectedSessionId && selected}
      {@const session = selected}
      <button type="button" class={quiet} onclick={() => renameSession(session)}>Rename</button>
      {#if session.role === 'owner'}
        <button type="button" class={quiet} onclick={() => (selectedIsArchived ? restoreSession(session) : archiveSession(session))}>
          {selectedIsArchived ? 'Restore' : 'Archive'}
        </button>
        <DangerLink onClick={() => deleteSession(session)}>Delete</DangerLink>
      {/if}
    {/if}
    {#if tierNames.length > 0}
      <!-- The harness sits beside the view's other model-level controls, not
           in the composer. -->
      <ComposerPicker
        icon="✳"
        chipVariant="primary"
        value={sessionTier}
        label={sessionTier || 'main'}
        options={tierOptions}
        meter={tierMeter}
        searchPlaceholder="Search tiers"
        menuClass="min-w-44"
        title="Model tier for this session"
        menuLabel="Model tier"
        onChange={(t) => (sessionTier = t)}
      />
    {/if}
  </div>
{/snippet}

{#snippet stageHeader()}
  {#if selectedAgent && current}
    <!-- No title: a selected session's name lives in the strip (claimed
         above); an unsaved one has no name worth a header. The row keeps the
         agent and the session-level controls. -->
    <StageHeader meta={`with ${current.label}`} actions={headerActions} />
  {/if}
{/snippet}

<RailSurface>
  <ConversationSidebar
    {agents}
    {conversations}
    {archived}
    {selectedAgent}
    selectedConversationId={selectedSessionId}
    {agentsLoading}
    {conversationsLoading}
    {agentsFailure}
    {conversationsFailure}
    {archivedFailure}
    onSelectAgent={selectAgent}
    onSelectConversation={selectConversation}
    onNewChat={newSession}
    onRowMenu={openSessionMenu}
    noun="session"
    newPerm="work.sessions"
    newTitle="New session: work alongside the agent on a document"
    emptyHint="A session is you and the agent working on one file together — its stream here, the document beside it. Reads are free; anything it writes to Google waits for your approval."
    collapseKey="work"
  />

  <Stage header={stageHeader}>
    {#if selectedAgent && current}
      <div class="flex h-full min-h-0">
        <div class="flex min-w-0 flex-1 flex-col">
          <!-- Above the composer, not instead of it: a session with no model
               behind it still opens, and the fix is offered here. -->
          <NoModelBump class="m-4 shrink-0" />
          <div class="min-h-0 flex-1">
            {#key selectedAgent}
              <ChatView
                agentModel={selectedAgent}
                agentLabel={current.label}
                conversationId={selectedSessionId}
                {newChatSignal}
                {onCreated}
                kind="work"
                minimal
                tier={sessionTier}
              />
            {/key}
          </div>
        </div>
        <!-- The document pane. Two-pane from the FIRST keystroke — same rule
             the plan's living document follows — so W3 fills this column in
             rather than re-laying the surface out around it. -->
        <div class="hidden min-w-0 basis-[44%] lg:flex">
          <div class="flex min-w-0 flex-1 flex-col border-l border-line-subtle">
            <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
              <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Document</span>
            </div>
            <!-- EmptyState, not a centred sentence: `full` carries the
                 dithered vignette, which is what keeps a 44% column that owns
                 half the stage from reading as a dead void before a file is
                 open. -->
            <EmptyState
              icon="◫"
              title="No file open"
              hint={`The file you and ${current.label} are working on opens here, beside the session.`}
            />
          </div>
        </div>
      </div>
    {:else if agentsLoading}
      <!-- The stage frame renders immediately while the fleet loads: the
           same two-pane layout (session column + 44% document column) it
           stands in for, so nothing pops or re-layouts when agents land. -->
      <div aria-hidden="true" class="flex h-full min-h-0">
        <div class="min-w-0 flex-1 overflow-hidden p-6">
          <div class="mx-auto flex w-full max-w-[var(--converse-width)] flex-col gap-4">
            <!-- Chat flattens onto the panel (spec §10) — the stand-in
                 blocks wear the same radius-8 panels the messages will. -->
            <Skeleton class="h-14 w-3/5 self-end rounded-lg" />
            <Skeleton class="h-24 w-4/5 rounded-lg" />
            <Skeleton class="h-12 w-1/2 self-end rounded-lg" />
            <Skeleton class="h-20 w-3/4 rounded-lg" />
          </div>
        </div>
        <div class="hidden min-w-0 basis-[44%] flex-col border-l border-line-subtle lg:flex">
          <div class="h-12 shrink-0 border-b border-line-subtle"></div>
          <div class="flex-1 p-4">
            <Skeleton class="h-full w-full rounded-lg" />
          </div>
        </div>
      </div>
    {:else if agentsFailure}
      <!-- "No agents available." is a statement about the FLEET. When the
           fleet read itself failed, the only honest thing to report is that. -->
      <div class="grid h-full place-items-center">
        <QueryError error={agentsFailure.error} title="Could not load your agents" onRetry={agentsFailure.retry} />
      </div>
    {:else}
      <!-- A statement about the FLEET, so it says what to do about it —
           bare "No agents available." named the absence and offered no way
           out of it. The action only appears for someone who can act on it:
           /agents is a Manage view, and offering a button that bounces is
           worse than offering none. -->
      <EmptyState
        icon="◇"
        title="No agents yet"
        hint="A work session is a conversation with one of your agents. Hire one and it shows up here."
        action={mayManageAgents.current ? hireAction : undefined}
      />
    {/if}
  </Stage>

  <ContextMenu {menu} />
</RailSurface>
