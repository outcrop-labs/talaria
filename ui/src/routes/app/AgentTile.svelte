<script lang="ts">
  import AgentItem, { type AgentFrame } from '@/components/fleet/AgentItem.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Panel from '@/components/ui/Panel.svelte'
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
    <Panel
      oncontextmenu={onContextMenu}
      class={cn(
        'relative flex h-full flex-col gap-4 p-5 transition-colors hover:border-line-strong hover:bg-hover has-[[data-card-open]:focus-visible]:border-line-strong has-[[data-card-open]:focus-visible]:bg-hover',
        !d.enabled && 'opacity-60',
      )}
    >
      <div class="flex items-start gap-2.5">
        <!-- 32px, centred on a one-line name and role: the width goes to the name. -->
        <Avatar name={d.displayName} class="mt-1 h-8 w-8" />
        <!-- The whole card opens the agent: the button's ::after covers it,
             and the menu and Start sit above that. The global focus rule is
             unlayered, so the button's own ring is forced off and the card's
             is drawn on the ::after. -->
        <button
          type="button"
          data-card-open
          onclick={openManage}
          class="min-w-0 flex-1 cursor-pointer text-left after:absolute after:inset-0 after:rounded-lg focus-visible:outline-none! focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-accent"
        >
          <span class="line-clamp-2 font-sans text-sm font-medium leading-5 text-fg" title={d.displayName}>{d.displayName}</span>
          <span class="line-clamp-2 font-sans text-[13px] leading-5 text-muted" title={d.role ?? undefined}>{d.role ?? 'No role yet'}</span>
        </button>
        <!-- No gap ahead of the cell: its own inset is the space between the name and the dots. -->
        <div class="relative z-10 -ml-2.5 -mt-1.5 shrink-0">
          <SettingsRowMenu label={`Actions for ${d.displayName}`} items={menuItems} />
        </div>
      </div>
      {#if roll?.state === 'failed' && roll.detail}
        <p class="line-clamp-2 font-sans text-[13px] leading-5 text-danger" title={roll.detail}>{roll.detail}</p>
      {/if}
      <div class="mt-auto flex min-h-8 items-center gap-2.5">
        <AgentStatusDot def={d} {containers} {brain} />
        <AgentRollChip {roll} class="relative z-10" />
        <span class="ml-auto flex min-w-0 items-center gap-2.5">
          {#if brainDown}
            <AgentBrainChip {brain} class="relative z-10" />
          {:else if model}
            <!-- One voice down the footers: a model the catalog does not name shows its id in the same type. -->
            <span class="min-w-0 truncate font-sans text-[13px] leading-5 text-muted" title={model}>{modelName ?? model}</span>
          {/if}
          <AgentControls def={d} {containers} {running} {roll} {act} {pending} />
        </span>
      </div>
    </Panel>
  {/snippet}
</AgentItem>
