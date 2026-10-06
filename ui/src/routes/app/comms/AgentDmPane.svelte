<script lang="ts">
  import { SquarePen } from '@lucide/svelte'
  import ChatView from '@/components/chat/ChatView.svelte'
  import NoModelBump from '@/components/setup/NoModelBump.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { agentStarKey } from '@/lib/comms-stars'
  import StarButton from './StarButton.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'

  // One agent's DM pane. Threads are selected in the sidebar (nested under the
  // agent); a fresh thread is the default — bounded context per topic.
  let {
    model,
    fleet,
    conversationId,
    newChatSignal,
    onNewThread,
    onCreated,
    working = false,
  }: {
    model: string
    fleet: { id: string; label: string; tiers?: string[] }[]
    conversationId: string | null
    newChatSignal: number
    onNewThread: () => void
    onCreated: (id: string) => void
    /** The open thread's agent is writing a reply right now. */
    working?: boolean
  } = $props()

  const agent = $derived(fleet.find((a) => a.id === model))
</script>

{#if agent}
  <div class="flex h-full min-h-0 flex-col">
    <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-5">
      <StarButton starKey={agentStarKey(model)} class="-ml-2" />
      <span class="text-sm font-semibold text-fg">◍ {agent.label}</span>
      {#if working}
        <!-- The same mark as the rail's thread row, beside the name while the
             agent writes this thread's reply. -->
        <span class="flex items-center" title={`${agent.label} is writing a reply`}>
          <WaitingMark site="comms/thread-working" size={12} class="text-accent" />
        </span>
      {/if}
      {#if conversationId === null}<span class="text-xs text-muted">new thread; history stays out of context</span>{/if}
      <span class="ml-auto"></span>
      <IconButton size="sm" title="New thread: fresh context" onclick={onNewThread}>
        <SquarePen size={16} />
      </IconButton>
    </header>
    <!-- Chat is the surface where a missing provider is most confusing — the
         thread opens, the composer works, and nothing comes back. -->
    <NoModelBump class="m-4 shrink-0" />
    <div class="min-h-0 flex-1">
      <ChatView
        agentModel={model}
        agentLabel={agent.label}
        tiers={agent.tiers ?? []}
        {conversationId}
        {newChatSignal}
        {onCreated}
      />
    </div>
  </div>
{/if}
