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
  import ArtifactEditor from './ArtifactEditor.svelte'
  import ConversationMembers from '@/components/chat/ConversationMembers.svelte'
  import LivingDoc from '@/components/chat/LivingDoc.svelte'
  import GoogleFilePane from '@/components/chat/GoogleFilePane.svelte'
  import KbCommentsPanel from './KbCommentsPanel.svelte'
  import { createQuery } from '@tanstack/svelte-query'
  import { getJson } from '@/lib/fetch-json'
  import { useSession } from '@/lib/session'
  import type { KbComment } from './knowledge.svelte'
  import {
    archiveConversation,
    deleteConversation,
    renameConversation,
    restoreConversation,
    useConversations,
    useConversationMembers,
    loadConversation,
    setPinnedFiles,
    type Conversation,
    type PinnedFile,
  } from '@/lib/conversations.svelte'
  import { userMentionInsert } from '@/components/chat/mentions.svelte'
  import { parseGoogleFileUrl } from '@/lib/google-embed'
  import { alert, prompt } from '@/components/ui/confirm.svelte'

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
  const session = useSession()
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
  // @mention the session's MEMBERS — the people a mention will actually reach.
  // Same rule as Plan: offering the whole org invites mentions that notify
  // nobody. A brand-new session has only you, so it is inert until shared.
  const membersQuery = useConversationMembers(() => selectedSessionId)
  const mentionables = $derived(
    (membersQuery.data?.members ?? [])
      .map((u) => ({
        insert: userMentionInsert({ name: u.name, email: u.email }),
        label: u.name ?? u.email ?? u.userId,
        sub: u.email ?? undefined,
      }))
      .filter((m) => m.insert),
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
  // THE PANE FOLLOWS THE AGENT'S WORK. The document the session last touched,
  // reported by ChatView from the landed turn's link chips — so opening a
  // session puts you back in front of the file it was about, and an agent that
  // creates a document mid-session brings it into view without anybody
  // clicking. Per-session `pinned_files` is the next turn of this: the column
  // exists, and it will let a person override what the agent chose.
  // A GOOGLE FILE PINNED TO THIS SESSION, if any — the pane's Tier A. Held
  // per session on the conversation row (`pinned_files`), so reopening a
  // session puts the same file back in front of you, and a collaborator who
  // opens it sees the file the session is about rather than an empty pane.
  let pinnedGoogle = $state<PinnedFile | null>(null)
  $effect(() => {
    const id = selectedSessionId
    if (!id) {
      pinnedGoogle = null
      return
    }
    // Read through the detail endpoint rather than the list: `pinned_files` is
    // a per-conversation column and the rail's rows do not carry it.
    void loadConversation(id).then((d) => {
      if (selectedSessionId !== id) return
      pinnedGoogle = (d?.conversation.pinnedFiles ?? []).find((f) => f.kind === 'google') ?? null
    })
  })
  // PASTE A LINK. The smallest honest way in: people already have the URL,
  // and a Drive browser is a bigger surface than this pane needs to prove
  // itself. The type is read from the URL's own shape — a link that is not a
  // Google file is refused here rather than pinned into a blank frame.
  const pinGoogle = async () => {
    const id = selectedSessionId
    if (!id) return
    const raw = await prompt({
      title: 'Open a Google file',
      message: 'Paste the link to a Google Doc, Sheet, Slides deck or Drive file.',
      placeholder: 'https://docs.google.com/document/d/…',
      confirmLabel: 'Open',
    })
    if (!raw?.trim()) return
    const hit = parseGoogleFileUrl(raw)
    if (!hit) {
      await alert({
        title: 'Not a Google file link',
        message: 'That did not look like a docs.google.com or drive.google.com file URL.',
      })
      return
    }
    const file: PinnedFile = { kind: 'google', id: hit.id, ...(hit.mime ? { mime: hit.mime } : {}) }
    if (await setPinnedFiles(id, [file])) pinnedGoogle = file
  }
  const unpinGoogle = () => {
    const id = selectedSessionId
    pinnedGoogle = null
    if (id) void setPinnedFiles(id, [])
  }
  let paneArtifactId = $state<string | null>(null)
  // COMMENTS ON THE DOCUMENT IN THE PANE. The pane is narrow, so they swap in
  // rather than sitting beside the editor — a 44% column split again would give
  // neither half enough room to be worth having.
  let showComments = $state(false)
  let livingDocId = $state<string | null>(null)
  // The artifact the comments belong to: whatever the pane is actually showing.
  // A pinned Google file has Google's own comments inside the embed, so this is
  // null there and the control is hidden.
  const commentTargetId = $derived(paneArtifactId ?? livingDocId)
  const commentsQuery = createQuery(() => {
    const id = commentTargetId
    return {
      queryKey: ['artifact-comments', id],
      enabled: !!id && showComments,
      queryFn: (): Promise<{ comments: KbComment[] }> =>
        getJson<{ comments: KbComment[] }>(`/api/artifacts/${id}/comments`),
    }
  })
  // Bumped when an agent turn lands; the living document asks for its new body
  // on the bump. The server also rewrites the document on a landed turn, and
  // its recency guard means whichever gets there first does the one rewrite.
  let turnSignal = $state(0)
  // Tools whose completion means the OPEN document changed underneath us. Only
  // the artifact writers: a Google write of someone else's file queues rather
  // than landing, so re-reading on it would show the unchanged document and
  // imply the write went through.
  const MUTATES_ARTIFACT = new Set([
    'create_document',
    'update_document',
    'create_sheet',
    'create_page',
    'save_image_artifact',
  ])
  const onToolDone = (name: string) => {
    // Invalidate rather than remount: the pane keeps its scroll and any
    // in-progress edit, and TanStack refetches the one key that changed.
    if (paneArtifactId && MUTATES_ARTIFACT.has(name)) {
      void qc.invalidateQueries({ queryKey: ['artifact', paneArtifactId] })
    }
  }
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
  // The claim is INERT right now: the dock era dropped the title row, and the
  // strip that would have read it is deleted, so nothing reads
  // `viewTitleClaim` today (see lib/view-title.svelte.ts). It stays because
  // the three sibling views all make it, and the day a surface wants view
  // titles again this one should not be the view that forgot.
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
    {#if selectedSessionId}
      <!-- Sharing and presence, the Plan idiom's own controls: a work session
           is a conversation with members, so it gets the same header row
           rather than a second one built for it. -->
      <ConversationMembers conversationId={selectedSessionId} />
    {/if}
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
                {onToolDone}
                {mentionables}
                onTurnComplete={() => (turnSignal += 1)}
                onDocumentTouched={(id) => (paneArtifactId = id)}
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
              {#if commentTargetId && !pinnedGoogle}
                <button
                  type="button"
                  class={`${quiet} ml-auto`}
                  onclick={() => (showComments = !showComments)}
                >
                  {showComments ? 'Document' : 'Comments'}
                </button>
              {/if}
              {#if paneArtifactId}
                <button type="button" class={quiet} onclick={() => (paneArtifactId = null)}>Close</button>
              {:else if pinnedGoogle}
                <button type="button" class={`${quiet} ml-auto`} onclick={unpinGoogle}>Unpin</button>
              {:else if selectedSessionId}
                <button type="button" class={`${quiet} ml-auto`} onclick={() => void pinGoogle()}>Open a Google file</button>
              {/if}
            </div>
            {#if showComments && commentTargetId}
              <!-- Swapped IN rather than beside: the pane is 44% of the stage
                   and splitting it again would leave neither half usable. A
                   pinned Google file never gets here — Google's own comments
                   live inside the embed. -->
              <KbCommentsPanel
                docId={commentTargetId}
                basePath="/api/artifacts"
                queryKey="artifact-comments"
                comments={commentsQuery.data?.comments ?? []}
                loadFailed={commentsQuery.isError}
                loadError={commentsQuery.error}
                onRetryLoad={() => void commentsQuery.refetch()}
                meId={session.data?.id ?? null}
                docOwnerId={null}
                pendingQuote={null}
                onQuoteConsumed={() => {}}
                onClose={() => (showComments = false)}
              />
            {:else if paneArtifactId}
              <!-- The agent opened or wrote a DIFFERENT file, so the pane
                   follows the work. Keyed on the id ALONE, not on a refresh
                   counter: a tool landing invalidates the artifact query
                   instead, so the editor keeps its scroll and any unsaved edit
                   while the new body arrives. -->
              {#key paneArtifactId}
                <div class="min-h-0 flex-1 overflow-hidden">
                  <ArtifactEditor id={paneArtifactId} onDeleted={() => (paneArtifactId = null)} />
                </div>
              {/key}
            {:else if pinnedGoogle}
              <!-- TIER A: the real Google editor, with Google's own presence
                   doing the collaboration. Pinned to the session, so it
                   outlives a reload and a collaborator sees the same file. -->
              <GoogleFilePane
                fileId={pinnedGoogle.id}
                mime={pinnedGoogle.mime ?? null}
                title={pinnedGoogle.title ?? null}
              />
            {:else if selectedSessionId}
              <!-- THE SESSION'S LIVING DOCUMENT is the default, which is the
                   whole draw of the surface: you talk, and the document builds
                   itself beside you. The server rewrites it when a turn lands
                   and `turnSignal` asks the pane for the new body. -->
              <LivingDoc
                conversationId={selectedSessionId}
                syncSignal={turnSignal}
                onDocId={(id) => (livingDocId = id)}
              />
            {:else}
              <!-- No session yet, so there is no document to build. `full`
                   carries the dithered vignette, which keeps a 44% column that
                   owns half the stage from reading as a dead void. -->
              <EmptyState
                icon="◫"
                title="No document yet"
                hint={`Say what you are working on and the document builds here as you and ${current.label} talk.`}
              />
            {/if}
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
