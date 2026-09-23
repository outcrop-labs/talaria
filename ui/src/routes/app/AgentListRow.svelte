<script lang="ts">
  import AgentItem, { type AgentFrame } from '@/components/fleet/AgentItem.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import { cn } from '@/lib/cn'
  import type { AgentBrainHealth, AgentContainers, AgentDef, LlmEndpoint } from '@/lib/fleet-defs'
  import AgentBrainChip from './AgentBrainChip.svelte'
  import AgentControls from './AgentControls.svelte'
  import AgentStatusDot from './AgentStatusDot.svelte'

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
</script>

<AgentItem def={d} {containers} {endpoints} {onDuplicate}>
  <!-- The frame: div, not li: the roster list renders these directly inside
       Materialize's container (a div carrying the panel frame + divide classes). -->
  {#snippet frame({ running, openManage, onContextMenu }: AgentFrame)}
    <div role="presentation" oncontextmenu={onContextMenu} class={cn('flex items-center gap-3 px-4 py-3 transition-colors dither-fill', !d.enabled && 'opacity-60')}>
      <AgentStatusDot def={d} {containers} />
      <Avatar name={d.displayName} class="h-7 w-7" />
      <button type="button" onclick={openManage} class="min-w-0 flex-1 text-left">
        <span class="text-sm font-medium text-fg">{d.displayName}</span>
        {#if d.role}<span class="ml-2 text-xs text-muted">{d.role}</span>{/if}
      </button>
      <AgentBrainChip {brain} />
      <AgentControls def={d} {running} onManage={openManage} {onDuplicate} />
    </div>
  {/snippet}
</AgentItem>