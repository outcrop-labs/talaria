<script lang="ts">
  import AgentItem, { type AgentFrame } from '@/components/fleet/AgentItem.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Panel from '@/components/ui/Panel.svelte'
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
  {#snippet frame({ running, openManage, onContextMenu }: AgentFrame)}
    <Panel oncontextmenu={onContextMenu} class={cn('flex flex-col gap-3', !d.enabled && 'opacity-60')}>
      <div class="flex items-center gap-2.5">
        <Avatar name={d.displayName} class="h-9 w-9" />
        <button type="button" onclick={openManage} class="min-w-0 flex-1 text-left">
          <div class="truncate text-sm font-medium text-fg">{d.displayName}</div>
          <div class="truncate text-xs text-muted">{d.role ?? `v${d.currentVersion}`}</div>
        </button>
        <AgentBrainChip {brain} />
        <AgentStatusDot def={d} {containers} />
      </div>
      <div class="flex justify-end">
        <AgentControls def={d} {running} onManage={openManage} {onDuplicate} />
      </div>
    </Panel>
  {/snippet}
</AgentItem>