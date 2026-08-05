<script lang="ts">
  import Avatar from '@/components/ui/Avatar.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import Panel from '@/components/ui/Panel.svelte'
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

<Panel oncontextmenu={am.onContextMenu} class={cn('flex flex-col gap-3', !d.enabled && 'opacity-60')}>
  <div class="flex items-center gap-2.5">
    <Avatar name={d.displayName} class="h-9 w-9" />
    <button type="button" onclick={() => (manage = true)} class="min-w-0 flex-1 text-left">
      <div class="truncate text-sm font-medium text-fg">{d.displayName}</div>
      <div class="truncate text-xs text-muted">{d.role ?? `v${d.currentVersion}`}</div>
    </button>
    <AgentBrainChip {brain} />
    <AgentStatusDot def={d} {containers} />
  </div>
  <div class="flex justify-end">
    <AgentControls def={d} {running} onManage={() => (manage = true)} {onDuplicate} />
  </div>
</Panel>
<ContextMenu menu={am.menu} />
{#if am.retiring}
  <AgentRetireModal def={d} onClose={() => (am.retiring = false)} onConfirm={() => void am.act('retire', 'retiring')} />
{/if}
{#if manage}
  <AgentManageModal open={manage} onClose={() => (manage = false)} def={d} {endpoints} isAdmin />
{/if}
