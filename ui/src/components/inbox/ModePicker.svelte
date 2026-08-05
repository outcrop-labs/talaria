<script lang="ts">
  import { chipSecondary, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  // Aliased: the local `pop` popover controller shadows motion's `pop`.
  import { pop as popIn, POPOVER } from '@/lib/motion'
  import type { AssistantMode } from './assistant-composer-controls'
  import { createAnchoredPopover } from './anchored-popover.svelte'

  const MODE_OPTIONS: Array<{ id: AssistantMode; label: string; detail: string }> = [
    { id: 'normal', label: 'Normal mode', detail: 'Balanced response with safe action proposals.' },
    { id: 'fast', label: 'Fast mode', detail: 'Prefer the quickest deterministic safe response.' },
    { id: 'plan', label: 'Plan mode', detail: 'Plan and clarify without proposing execution.' },
  ]

  let { value, onChange }: { value: AssistantMode; onChange: (mode: AssistantMode) => void } = $props()

  let open = $state(false)
  const pop = createAnchoredPopover(
    () => open,
    (next) => (open = next),
  )
</script>

<button
  bind:this={pop.button}
  type="button"
  onclick={() => (open = !open)}
  class={cn(chipSecondary, 'w-24 justify-center')}
  title="Assistant response mode"
>
  {value} mode
</button>
{#if open && pop.position}
  <div
    use:portal
    bind:this={pop.panel}
    in:popIn={POPOVER}
    class={cn(popPanel, 'fixed z-[70] w-64')}
    style:left="{pop.position.left}px"
    style:bottom="{pop.position.bottom}px"
  >
    <div class={popHeader}>Response mode</div>
    {#each MODE_OPTIONS as option (option.id)}
      <button
        type="button"
        onclick={() => {
          onChange(option.id)
          open = false
        }}
        class={cn(popRow, option.id === value ? popRowSelected : 'text-muted')}
      >
        <span class="min-w-0 flex-1">
          <span class="block text-fg">{option.label}</span>
          <span class="block text-[11px] leading-4 text-ink-dim">{option.detail}</span>
        </span>
      </button>
    {/each}
  </div>
{/if}
