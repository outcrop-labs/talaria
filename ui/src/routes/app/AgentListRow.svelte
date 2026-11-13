<script lang="ts">
  import AgentItem, { type AgentFrame } from '@/components/fleet/AgentItem.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import { cn } from '@/lib/cn'
  import type { AgentBrainHealth, AgentContainers, AgentDef, FleetRollRow, LlmEndpoint } from '@/lib/fleet-defs'
  import AgentBrainChip from './AgentBrainChip.svelte'
  import AgentControls from './AgentControls.svelte'
  import AgentRollChip from './AgentRollChip.svelte'
  import AgentStatusDot from './AgentStatusDot.svelte'

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

  const model = $derived(d.latest?.config?.main?.model ?? null)
  const brainDown = $derived(brain?.targets.some((t) => !t.ok) ?? false)
</script>

<AgentItem def={d} {containers} {endpoints} {onDuplicate} {roll}>
  <!-- A div, not an li: the roster renders these straight into Materialize's
       container, which carries the card frame. -->
  {#snippet frame({ running, openManage, onContextMenu, menuItems, act, pending }: AgentFrame)}
    <div
      role="presentation"
      oncontextmenu={onContextMenu}
      class={cn('relative flex min-h-[60px] items-center gap-4 px-5 py-3 transition-colors hover:bg-hover', !d.enabled && 'opacity-60')}
    >
      <Avatar name={d.displayName} class="h-8 w-8" />
      <button
        type="button"
        onclick={openManage}
        class="min-w-0 flex-1 text-left after:absolute after:inset-0 focus-visible:outline-none focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-accent"
      >
        <span class="block truncate font-sans text-sm font-medium leading-5 text-fg">{d.displayName}</span>
        <span class="block truncate font-sans text-[13px] leading-5 text-muted">{d.role ?? 'No role yet'}</span>
      </button>
      <!-- The row has the width for the failure sentence beside the chip. -->
      <AgentRollChip {roll} sentence class="relative z-10" />
      <span class="hidden w-44 shrink-0 items-center lg:flex">
        {#if brainDown}
          <AgentBrainChip {brain} class="relative z-10" />
        {:else if model}
          <span class="min-w-0 truncate font-mono text-xs text-muted" title={model}>{model}</span>
        {/if}
      </span>
      <span class="w-24 shrink-0"><AgentStatusDot def={d} {containers} /></span>
      <span class="flex min-w-[104px] shrink-0 items-center justify-end gap-1">
        <AgentControls def={d} {containers} {running} {roll} {act} {pending} />
        <span class="relative z-10"><SettingsRowMenu label={`Actions for ${d.displayName}`} items={menuItems} /></span>
      </span>
    </div>
  {/snippet}
</AgentItem>
