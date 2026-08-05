<script lang="ts">
  import Avatar from '@/components/ui/Avatar.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { cn } from '@/lib/cn'
  import AgentManageModal from '@/components/fleet/AgentManageModal.svelte'
  import type { AgentBrainHealth, AgentContainers, AgentDef, LlmEndpoint } from '@/lib/fleet-defs'
  import AgentBrainChip from './AgentBrainChip.svelte'
  import AgentControls from './AgentControls.svelte'
  import AgentRetireModal from './AgentRetireModal.svelte'
  import AgentStatusDot from './AgentStatusDot.svelte'
  import { healthOf, useAgentMenu } from './agents.svelte'

  let {
    def: d,
    containers,
    endpoints,
    brain,
    onDuplicate,
  }: {
    def: AgentDef
    containers: AgentContainers | null
    endpoints: LlmEndpoint[]
    brain?: AgentBrainHealth
    onDuplicate: () => void
  } = $props()

  let manage = $state(false)
  const running = $derived(healthOf(d, containers).running)
  const am = useAgentMenu(() => d, () => running, () => (manage = true), () => onDuplicate())
</script>

<li oncontextmenu={am.onContextMenu} class={cn('flex items-center gap-3 px-4 py-3 transition-colors hover:bg-hover', !d.enabled && 'opacity-60')}>
  <AgentStatusDot def={d} {containers} />
  <Avatar name={d.displayName} class="h-7 w-7" />
  <button type="button" onclick={() => (manage = true)} class="min-w-0 flex-1 text-left">
    <span class="text-sm font-medium text-fg">{d.displayName}</span>
    {#if d.role}<span class="ml-2 text-xs text-muted">{d.role}</span>{/if}
  </button>
  <AgentBrainChip {brain} />
  <AgentControls def={d} {running} onManage={() => (manage = true)} {onDuplicate} />
</li>
<ContextMenu menu={am.menu} />
{#if am.retiring}
  <AgentRetireModal def={d} onClose={() => (am.retiring = false)} onConfirm={() => void am.act('retire', 'retiring')} />
{/if}
{#if manage}
  <AgentManageModal open={manage} onClose={() => (manage = false)} def={d} {endpoints} isAdmin />
{/if}
