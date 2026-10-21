<script lang="ts">
  import AgentItem, { type AgentFrame } from '@/components/fleet/AgentItem.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import { cn } from '@/lib/cn'
  import type { AgentBrainHealth, AgentContainers, AgentDef, FleetRollRow, LlmEndpoint } from '@/lib/fleet-defs'
  import AgentDraftChip from './AgentDraftChip.svelte'
  import AgentBrainChip from './AgentBrainChip.svelte'
  import AgentControls from './AgentControls.svelte'
  import AgentRollChip from './AgentRollChip.svelte'
  import AgentStatusDot from './AgentStatusDot.svelte'
  import { healthOf } from './agents.svelte'

  let {
    def: d,
    containers,
    endpoints,
    brain,
    roll,
    onDuplicate,
  }: {
    def: AgentDef
    containers: AgentContainers | null
    endpoints: LlmEndpoint[]
    brain?: AgentBrainHealth
    roll?: FleetRollRow
    onDuplicate: () => void
  } = $props()
</script>

<AgentItem def={d} {containers} {endpoints} {onDuplicate} {roll}>
  {#snippet frame({ running, openManage, onContextMenu, rolling }: AgentFrame)}
    <Panel oncontextmenu={onContextMenu} class={cn('flex flex-col gap-3', !d.enabled && 'opacity-60')}>
      <div class="flex items-center gap-2.5">
        <Avatar name={d.displayName} class="h-9 w-9" />
        <button type="button" onclick={openManage} class="min-w-0 flex-1 text-left">
          <div class="truncate text-sm font-medium text-fg">{d.displayName}</div>
          <div class="truncate text-xs text-muted">{d.role ?? `v${d.currentVersion}`}</div>
        </button>
        {#if healthOf(d, containers).health === 'draft'}<AgentDraftChip />{/if}
        <AgentBrainChip {brain} />
        <AgentRollChip {roll} />
        <AgentStatusDot def={d} {containers} />
      </div>
      {#if roll?.state === 'failed' && roll.detail}
        <!-- The tile's header has no room for a sentence; the failure gets
             its own line, where a reader looking for the why will find it. -->
        <p class="truncate font-sans text-xs text-danger" title={roll.detail}>{roll.detail}</p>
      {/if}
      <div class="flex justify-end">
        <AgentControls def={d} {containers} {running} {rolling} onManage={openManage} {onDuplicate} />
      </div>
    </Panel>
  {/snippet}
</AgentItem>