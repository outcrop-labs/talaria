<script lang="ts">
  // The composer's send affordance (Mercury, spec §7). Send is the gold 36×36
  // tile pinned to the END of the control rail — outside the prompt well. It
  // is the rail's one submit tile: while a reply streams, ChatComposer swaps
  // it for the stop square (see its `onStop`) rather than showing both.
  // (Enter still sends everywhere.)
  import { ArrowUp } from '@lucide/svelte'
  import type { LucideIcon } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'

  /** The accent submit tile: 36×36, radius 6, dark up-arrow on gold. Disabled
   *  stays in the accent family — a soft gold tile, never the grey raised
   *  tile other chrome controls use. `icon` swaps the arrow for surfaces
   *  whose submit MEANS something other than "send" (Research: the same
   *  telescope the rail names it by). */
  let {
    onClick,
    enabled,
    title = 'Send (⏎)',
    icon,
    class: className,
  }: {
    onClick: () => void
    enabled: boolean
    title?: string
    icon?: LucideIcon
    class?: string
  } = $props()
</script>

<button
  type="button"
  {title}
  aria-label={title}
  disabled={!enabled}
  onclick={onClick}
  class={cn(
    'grid h-9 w-9 shrink-0 place-items-center rounded-md transition-colors',
    enabled
      ? 'bg-accent text-[color:var(--theme-bg)] hover:bg-[color:var(--theme-accent-secondary)]'
      : 'border border-accent/40 bg-accent-soft text-accent',
    focusGold,
    className,
  )}
>
  {#if icon}
    {@const Glyph = icon}
    <Glyph size={16} strokeWidth={2.25} />
  {:else}
    <ArrowUp size={16} strokeWidth={2.25} />
  {/if}
</button>
