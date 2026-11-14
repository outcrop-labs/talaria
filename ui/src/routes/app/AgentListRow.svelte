<script lang="ts" module>
  // The roster list's caption row and each row's cells share these widths.
  export const LIST_MODEL = 'hidden w-44 shrink-0 lg:block'
  export const LIST_STATUS = 'w-24 shrink-0'
  export const LIST_ACTION = 'flex min-w-14 shrink-0 justify-end'
</script>

<script lang="ts">
  import AgentItem, { type AgentFrame } from '@/components/fleet/AgentItem.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
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
    modelName,
    onDuplicate,
  }: {
    def: AgentDef
    containers: AgentContainers | null
    endpoints: LlmEndpoint[]
    brain?: AgentBrainHealth
    roll?: FleetRollRow
    /** The catalog's name for the agent's main model, when it has one. */
    modelName?: string
    onDuplicate: () => void
  } = $props()

  const model = $derived(d.latest?.config?.main?.model ?? null)
  const brainDown = $derived(brain?.targets.some((t) => !t.ok) ?? false)
</script>

<AgentItem def={d} {containers} {endpoints} {onDuplicate} {roll}>
  {#snippet frame({ running, openManage, onContextMenu, menuItems, act, pending }: AgentFrame)}
    <div role="presentation" oncontextmenu={onContextMenu} class={cn(!d.enabled && 'opacity-60')}>
      <SettingsRow hint={d.role ?? 'No role yet'} onOpen={openManage} openLabel={`${d.displayName} ${d.role ?? 'No role yet'}`} class="py-3">
        {#snippet lead()}<Avatar name={d.displayName} class="h-8 w-8" />{/snippet}
        {#snippet label()}<span class="line-clamp-2 whitespace-normal">{d.displayName}</span>{/snippet}
        <!-- The row has the width for the failure sentence beside the chip. -->
        {#snippet meta()}<AgentRollChip {roll} sentence class="pointer-events-auto" />{/snippet}
        <span class={cn(LIST_MODEL, 'truncate font-sans text-[13px] leading-5 text-muted')} title={brainDown ? undefined : (model ?? undefined)}>
          {#if brainDown}
            <AgentBrainChip {brain} class="pointer-events-auto" />
          {:else if model}
            {modelName ?? model}
          {/if}
        </span>
        <span class={LIST_STATUS}><AgentStatusDot def={d} {containers} {brain} /></span>
        <span class={LIST_ACTION}><AgentControls def={d} {containers} {running} {roll} {act} {pending} /></span>
        <SettingsRowMenu label={`Actions for ${d.displayName}`} items={menuItems} />
      </SettingsRow>
    </div>
  {/snippet}
</AgentItem>
