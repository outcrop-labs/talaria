<script lang="ts">
  // DRAFT TICKETS — the control that turns a conversation into reviewable
  // tickets, with the review modal behind it. It belongs to the DRAFT JOB
  // rather than to a surface: drafts pair to the conversation server-side
  // (plan-drafts.svelte.ts), so wherever this button is hosted it reports the
  // same three states and reopens to wherever the job got to — including after
  // a reload, which is what the hydrate below is for.
  import { ListChecks } from '@lucide/svelte'
  import PlanModal from '@/components/chat/PlanModal.svelte'
  import { hydratePlanDraft, planDraft } from '@/components/chat/plan-drafts.svelte'
  import Button from '@/components/ui/Button.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { useAgents } from '@/lib/agents'
  import { loadConversation } from '@/lib/conversations.svelte'

  let {
    conversationId,
  }: {
    /** The conversation the drafts pair to. Null is a real state — nothing is
     *  selected, or the first turn has not saved a conversation yet — and it
     *  is the button's disabled reading, with the title saying why. */
    conversationId: string | null
  } = $props()

  let open = $state(false)

  // WHO DRAFTS: the conversation's own agent. The server picks it for itself
  // (plans_id_draft.rs reads the conversation's agent and ignores any the body
  // names), but the modal still needs it by name — its planner row, its tier
  // picker and the label the drafting state reads all come off the roster — so
  // the button resolves it from the conversation rather than making every host
  // pass its own selection down. Read through the detail endpoint because the
  // rail's rows are a different query on every surface.
  const fleet = useAgents()
  let agentModel = $state<string | null>(null)
  $effect(() => {
    const id = conversationId
    if (!id) {
      agentModel = null
      return
    }
    void loadConversation(id).then((d) => {
      if (conversationId !== id) return
      agentModel = d?.conversation.agentModel ?? null
    })
  })
  const agent = $derived(fleet.data?.agents.find((a) => a.id === agentModel))

  // The conversation's ticket-draft job, if any — drafting (and the review
  // that lands after it) survives leaving the modal or the surface, and this
  // button is that pairing made visible.
  const job = $derived(conversationId ? planDraft(conversationId) : undefined)
  const draftTitle = $derived(
    !conversationId
      ? 'Start a plan first. Tickets draft from its conversation.'
      : job?.status === 'drafting'
        ? 'Drafting continues in the background. Reopen to watch.'
        : job?.status === 'ready'
          ? 'The drafted tickets are waiting for your review'
          : 'Turn this plan into tickets to review',
  )

  // Drafts pair to the conversation SERVER-side: arriving at one asks what is
  // paired to it, so a reload lands back on an in-flight draft ("Drafting…")
  // or a finished one ("Review drafts") with nothing lost.
  $effect(() => {
    const id = conversationId
    if (!id) return
    void hydratePlanDraft(id, `/api/plans/${id}/draft`)
  })
</script>

<!-- The payoff, dressed like it: this is what a planning surface exists for,
     so the action takes the gold primary (spec §8) and outranks the chips
     beside it. While the job drafts it says so (the waiting mark stands in for
     the icon), and once drafts land it becomes the way back to them.
     `primary`'s disabled reading is the raised tile — and the explanatory
     title lives on a WRAPPER because the button drops pointer-events when
     disabled; hover falls through to the span, which is what keeps the "why is
     it waiting" copy reachable. -->
<span class="inline-flex" title={draftTitle}>
  <Button size="sm" disabled={!conversationId} onclick={() => (open = true)}>
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

{#if conversationId && agent && open}
  <!-- Mount-gated, so each open starts the review walk where the job is
       rather than where the last visit left the modal. It waits on the
       resolved agent as well as the click: a modal whose planner row is empty
       cannot draft (its own Draft button stays disabled), and the roster read
       is a cached one on any surface that already lists its agents. The modal
       portals to <body>, so standing it here beside the button is safe. -->
  <PlanModal
    {open}
    onClose={() => (open = false)}
    planId={conversationId}
    draftUrl={`/api/plans/${conversationId}/draft`}
    agents={[agent]}
  />
{/if}
