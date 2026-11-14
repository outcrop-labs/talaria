<script lang="ts">
  import AgentItem, { type AgentFrame } from '@/components/fleet/AgentItem.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import ModelName from '@/components/models/ModelName.svelte'
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

  // A name that takes two lines leaves the role one, so no card's text runs
  // past three lines and every card in the grid is the same height.
  const NAME_LINE_PX = 20
  let nameEl = $state<HTMLElement>()
  let nameWraps = $state(false)
  $effect(() => {
    const el = nameEl
    if (!el) return
    const observer = new ResizeObserver(() => (nameWraps = el.offsetHeight > NAME_LINE_PX * 1.5))
    observer.observe(el)
    return () => observer.disconnect()
  })
</script>

<AgentItem def={d} {containers} {endpoints} {onDuplicate} {roll}>
  {#snippet frame({ running, openManage, onContextMenu, menuItems, act, pending }: AgentFrame)}
    <Panel
      oncontextmenu={onContextMenu}
      class={cn('relative flex h-full flex-col gap-4 p-5 transition-colors hover:border-line-strong', !d.enabled && 'opacity-60')}
    >
      <div class="flex items-start gap-3">
        <Avatar name={d.displayName} class="h-10 w-10" />
        <!-- The whole card opens the agent: the button's ::after covers it,
             and the menu and Start sit above that. -->
        <button
          type="button"
          onclick={openManage}
          class="min-w-0 flex-1 text-left after:absolute after:inset-0 after:rounded-lg focus-visible:outline-none focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-accent"
        >
          <span bind:this={nameEl} class="line-clamp-2 font-sans text-sm font-medium leading-5 text-fg" title={d.displayName}>{d.displayName}</span>
          <span class={cn('font-sans text-[13px] leading-5 text-muted', nameWraps ? 'line-clamp-1' : 'line-clamp-2')} title={d.role ?? undefined}>{d.role ?? 'No role yet'}</span>
        </button>
        <div class="relative z-10 -mt-1.5 shrink-0">
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
            <ModelName id={model} name={modelName} quiet />
          {/if}
          <AgentControls def={d} {containers} {running} {roll} {act} {pending} />
        </span>
      </div>
    </Panel>
  {/snippet}
</AgentItem>
