<script lang="ts">
  import { CircleHelp } from '@lucide/svelte'
  import { popPanel, tileBase } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import { createAnchoredPopover } from './anchored-popover.svelte'

  let open = $state(false)
  const pop = createAnchoredPopover(
    () => open,
    (next) => (open = next),
  )
</script>

<button bind:this={pop.button} type="button" onclick={() => (open = !open)} class={tileBase} title="Composer help">
  <CircleHelp size={14} />
</button>
{#if open && pop.position}
  <div
    use:portal
    bind:this={pop.panel}
    class={cn(popPanel, 'fixed z-[70] w-64 p-3')}
    style:left="{pop.position.left}px"
    style:bottom="{pop.position.bottom}px"
  >
    <div class="font-sans text-[13px] font-medium text-fg">Assistant composer</div>
    <div class="mt-2 space-y-1.5 font-sans text-[11px] leading-4 text-muted">
      <p>Enter sends. Shift + Enter adds a line.</p>
      <p>Attached decisions keep Talaria's action allowlist and confirmation rules.</p>
      <p>Plan mode never proposes execution.</p>
    </div>
  </div>
{/if}
