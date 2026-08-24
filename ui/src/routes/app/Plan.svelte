<script lang="ts">
  import { pathId } from '@/lib/route-tabs'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigate, route } from '@/router'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import ChatView from '@/components/chat/ChatView.svelte'
  import { LayoutTemplate, ListChecks } from '@lucide/svelte'
  import ConversationSidebar from '@/components/chat/ConversationSidebar.svelte'
  import type { SidebarFailure } from '@/components/chat/conversation-sidebar'
  import RailSurface from '@/components/app/RailSurface.svelte'
  import Stage from '@/components/app/Stage.svelte'
  import StageHeader from '@/components/app/StageHeader.svelte'
  import PlanModal from '@/components/chat/PlanModal.svelte'
  import { hydratePlanDraft, planDraft } from '@/components/chat/plan-drafts.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import PlanDoc from '@/components/chat/PlanDoc.svelte'
  import PlanDocSkeleton from '@/components/chat/PlanDocSkeleton.svelte'
  import TierPicker from '@/components/chat/TierPicker.svelte'
  import ComposerPicker from '@/components/chat/ComposerPicker.svelte'
  import { userMentionInsert } from '@/components/chat/mentions.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useAgents } from '@/lib/agents'
  import { useTemplates } from '@/lib/templates'
  import { useStickyAgent } from '@/lib/sticky-agent.svelte'
  import NoModelBump from '@/components/setup/NoModelBump.svelte'
  import { usePlanMembers, useConversations, type Conversation } from '@/lib/conversations.svelte'
  import PlanMembers from './plan/PlanMembers.svelte'

  // Plan surface: think through the work with an agent, then draft tickets and
  // send them to a board. A plan is a durable MULTIPLAYER conversation
  // (kind='plan'): the owner shares it with teammates, everyone talks to the
  // same agent and living document, presence shows who's here now.

  const qc = useQueryClient()
  // Both reads keep their query object: the rail and the stage each render a
  // sentence ("No plans yet with this agent.", "No agents available.") that is
  // only true of a request that SUCCEEDED and came back empty.
  const fleetQuery = useAgents()
  const agentsLoading = $derived(fleetQuery.isLoading)
  const agents = $derived(fleetQuery.data?.agents ?? [])
  const conversationsQuery = useConversations('plan')
  const conversations = $derived(conversationsQuery.data ?? [])
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
  const sticky = useStickyAgent('plan', () => agents)
  const selectedAgent = $derived(sticky.selected)
  const pickAgent = sticky.select
  // THE URL IS THE PLAN SELECTION (/plan/<id>) — linkable, back/forward-able.
  const selectedConversationId = $derived(pathId(route.pathname, '/plan'))
  const setSelectedConversationId = (id: string | null, opts: { replace?: boolean } = {}) => {
    if (id) void navigate('/plan/:planId', { params: { planId: id }, replace: opts.replace })
    else void navigate('/plan', { replace: opts.replace })
  }
  // @mention the plan's MEMBERS — the people a mention will actually reach.
  // (Offering the whole org invited mentions that silently notified nobody.)
  // Tokens mirror the server's; a brand-new plan has only you, so it's inert.
  const membersQuery = usePlanMembers(() => selectedConversationId)
  const mentionables = $derived(
    (membersQuery.data?.members ?? [])
      .map((u) => ({ insert: userMentionInsert({ name: u.name, email: u.email }), label: u.name ?? u.email ?? u.userId, sub: u.email ?? undefined }))
      .filter((m) => m.insert),
  )
  let newChatSignal = $state(0)
  let planOpen = $state(false)
  // The model-tier pick lives in the VIEW, not the composer: the plan's chat
  // surface is attach + text + submit only, and the sidebar owns the agent.
  // A new conversation starts on the agent's main model ('').
  let planTier = $state('')
  // The template a NEW plan's living doc seeds from ('' = automatic: the plan
  // agent's bound plan template). Locked in when the first turn creates the plan.
  let templateId = $state('')
  // Defaulted, a failed template read made the picker DISAPPEAR (the
  // `planTemplates.length > 0 &&` below), so a new plan silently seeded from
  // the agent default with no sign the choice had ever existed.
  const templatesList = listQuery(useTemplates(), { title: 'Could not load plan templates', variant: 'inline' })
  const templatesLoading = $derived(templatesList.pending)
  const planTemplates = $derived(templatesList.rows.filter((t) => t.kind === 'plan'))
  // The picker's rows: Automatic first (it is the default and the most common
  // choice), the library after. The sub line carries what the old option's
  // parenthetical spelled out.
  const templateOptions = $derived([
    { value: '', label: 'Automatic', sub: 'agent default' },
    ...planTemplates.map((t) => ({ value: t.id, label: t.name })),
  ])
  // Bumped when an agent turn lands; the doc pane syncs itself on it.
  let turnSignal = $state(0)

  const selectConversation = (c: Conversation) => {
    pickAgent(c.agentModel)
    setSelectedConversationId(c.id)
  }
  const selectAgent = (agentModel: string) => {
    pickAgent(agentModel)
    setSelectedConversationId(null)
    newChatSignal += 1
    templateId = ''
    planTier = ''
  }
  const newPlan = () => {
    if (selectedAgent) selectAgent(selectedAgent)
  }
  const onCreated = (id: string) => {
    setSelectedConversationId(id)
    void qc.invalidateQueries({ queryKey: ['conversations'] })
  }

  // An inbound ?p= may reference a plan whose AGENT isn't the sticky pick —
  // align the agent once the plan list resolves (URL itself stays put).
  $effect(() => {
    if (!selectedConversationId) return
    const target = conversations.find((c) => c.id === selectedConversationId)
    if (target && target.agentModel !== sticky.selected) pickAgent(target.agentModel)
  })

  const current = $derived(agents.find((a) => a.id === selectedAgent))

  const selected = $derived(conversations.find((c) => c.id === selectedConversationId) ?? null)

  // The selected plan's ticket-draft job, if any — drafts PAIR to the plan,
  // so drafting (and the review that lands after it) survives leaving the
  // modal or the surface. The header button is the pairing made visible.
  const job = $derived(selectedConversationId ? planDraft(selectedConversationId) : undefined)
  const draftTitle = $derived(
    !selectedConversationId
      ? 'Start a plan first — tickets draft from its conversation'
      : job?.status === 'drafting'
        ? 'Drafting continues in the background — reopen to watch'
        : job?.status === 'ready'
          ? 'The drafted tickets are waiting for your review'
          : 'Turn this plan into tickets to review',
  )

  // The selected plan owns the strip: its title becomes the view title and
  // the crumb's last segment (WORK / PLAN / <plan>). Effect-claimed like
  // Research/Boards — the list query lands after mount, and the pathname key
  // keeps the fallback honest on deep links. An unsaved "New plan" claims
  // nothing: it isn't a place yet.
  $effect(() => {
    if (!selected) return
    const name = selected.title || 'Untitled plan'
    claimViewTitle(name, { trail: [name] })
  })

  // Drafts pair to the plan SERVER-side: arriving at a conversation asks what
  // is paired to it, so a reload lands back on an in-flight draft
  // ("Drafting…") or a finished one ("Review drafts") with nothing lost.
  $effect(() => {
    const id = selectedConversationId
    if (!id) return
    void hydratePlanDraft(id, `/api/plan/${id}/draft`)
  })
</script>

{#snippet headerActions()}
  <div class="flex items-center gap-3">
    {#if selectedConversationId}
      <PlanMembers planId={selectedConversationId} />
    {:else if templatesLoading}
      <!-- Hold the template picker's spot so the header doesn't
           re-layout when templates land. -->
      <Skeleton class="h-9 w-40 rounded-md" />
    {:else if templatesList.failed}
      <!-- The template read failed: say so inline. Falling through
           to the picker would hide the failure behind a list that
           only looks empty. -->
      {#if templatesList.notice}<QueryError {...templatesList.notice} />{/if}
    {:else if planTemplates.length > 0}
      <!-- The template pick in the header's own language — the §7 chip
           TierPicker beside it already speaks. It was a native <select> under
           a mono label: its fixed-width trigger overflowed on long template
           names and its OS-drawn option list clashed with everything around
           it. The chip truncates instead, and the popover opens DOWN
           ('bottom' placement) — the header sits at the top of the stage, so
           the composer-style upward anchor would fly off-screen. -->
      <ComposerPicker
        icon={LayoutTemplate}
        value={templateId}
        options={templateOptions}
        onChange={(v) => (templateId = v)}
        title="The structure the living document starts from. Automatic uses the agent's bound plan template."
        menuLabel="Template"
        placement="bottom"
      />
    {/if}
    {#if (current?.tiers ?? []).length > 0}
      <!-- The harness sits beside the view's other model-level controls, not
           in the composer. -->
      <TierPicker tiers={current!.tiers ?? []} value={planTier} onChange={(t) => (planTier = t)} />
    {/if}
    <!-- The payoff, dressed like it: this surface exists to turn planning
         into tickets, so the action takes the gold primary (spec §8) and
         outranks the chips beside it. It is also the drafts' pairing made
         visible: while the job drafts it says so (the waiting mark stands in
         for the icon), and once drafts land it becomes the way back to them.
         `primary`'s disabled reading is the raised tile — and the explanatory
         title lives on a WRAPPER because the button drops pointer-events when
         disabled; hover falls through to the span, which is what keeps the
         "why is it waiting" copy reachable. -->
    <span class="inline-flex" title={draftTitle}>
      <Button size="sm" disabled={!selectedConversationId} onclick={() => (planOpen = true)}>
        {#if job?.status === 'drafting'}
          <WaitingMark site="plan/draft" size={13} class="text-[color:var(--theme-bg)]" />
          Drafting…
        {:else if job?.status === 'ready'}
          <ListChecks size={14} />
          Review drafts
        {:else}
          <ListChecks size={14} />
          Draft tickets
        {/if}
      </Button>
    </span>
  </div>
{/snippet}

{#snippet stageHeader()}
  {#if selectedAgent && current}
    <!-- No title: a selected plan's name lives in the strip (claimed above);
         an unsaved one has no name worth a header. The row keeps the agent
         and the plan-level controls. -->
    <StageHeader meta={`with ${current.label}`} actions={headerActions} />
  {/if}
{/snippet}

<RailSurface>
  <ConversationSidebar
    {agents}
    {conversations}
    {selectedAgent}
    {selectedConversationId}
    {agentsLoading}
    {conversationsLoading}
    {agentsFailure}
    {conversationsFailure}
    onSelectAgent={selectAgent}
    onSelectConversation={selectConversation}
    onNewChat={newPlan}
  />

  <Stage header={stageHeader}>
    {#if selectedAgent && current}
      <div class="flex h-full min-h-0">
        <div class="flex min-w-0 flex-1 flex-col">
          <!-- Above the composer, not instead of it: a plan with no model
               behind it still opens, and the fix is offered here. -->
          <NoModelBump class="m-4 shrink-0" />
          <div class="min-h-0 flex-1">
            {#key selectedAgent}
              <ChatView
                agentModel={selectedAgent}
                agentLabel={current.label}
                conversationId={selectedConversationId}
                {newChatSignal}
                {onCreated}
                kind="plan"
                minimal
                tier={planTier}
                templateId={templateId || null}
                {mentionables}
                onTurnComplete={() => (turnSignal += 1)}
              />
            {/key}
          </div>
        </div>
        <!-- The plan's living document — side by side from the FIRST
             keystroke, so the surface never re-layouts mid-thought. Before
             the conversation exists, the pane shows what will grow here. -->
        <div class="hidden min-w-0 basis-[44%] lg:flex">
          {#if selectedConversationId}
            <PlanDoc planId={selectedConversationId} planTitle={selected?.title ?? null} syncSignal={turnSignal} />
          {:else}
            <div class="flex min-w-0 flex-1 flex-col border-l border-line-subtle">
              <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
                <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Plan document</span>
              </div>
              <div class="grid flex-1 place-items-center p-8 text-center">
                <div class="max-w-56 font-sans text-xs leading-relaxed text-muted">
                  The living document builds here as you talk. {current.label} keeps it current, and you can edit it directly.
                </div>
              </div>
            </div>
          {/if}
        </div>
      </div>
    {:else if agentsLoading}
      <!-- The stage frame renders immediately while the fleet loads: the
           same two-pane layout (chat column + 44% doc column) it stands in
           for, so nothing pops or re-layouts when agents land. -->
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
        <div class="hidden min-w-0 basis-[44%] border-l border-line-subtle lg:flex">
          <PlanDocSkeleton />
        </div>
      </div>
    {:else if agentsFailure}
      <!-- "No agents available." is a statement about the FLEET. When the
           fleet read itself failed, the only honest thing to report is that. -->
      <div class="grid h-full place-items-center">
        <QueryError error={agentsFailure.error} title="Could not load your agents" onRetry={agentsFailure.retry} />
      </div>
    {:else}
      <div class="grid h-full place-items-center font-sans text-sm text-muted">No agents available.</div>
    {/if}
  </Stage>

  {#if selectedConversationId && current && planOpen}
    <PlanModal
      open={planOpen}
      onClose={() => (planOpen = false)}
      planId={selectedConversationId}
      draftUrl={`/api/plan/${selectedConversationId}/draft`}
      agents={[current]}
    />
  {/if}
</RailSurface>
