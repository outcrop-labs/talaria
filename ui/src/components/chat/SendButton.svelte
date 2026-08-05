<script lang="ts">
  // The composer's send affordance (Mercury, spec §7). Send is the gold 36×36
  // tile that lives INSIDE the prompt well, top-right; Enter still sends
  // everywhere and the KeyHint chip on the control rail says so. (Stop, its
  // streaming-time sibling, lives in StopButton.svelte.)
  import { ArrowUp } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'

  /** The gold submit tile: 36×36, radius 6, dark up-arrow on gold. Disabled
   *  reads as a raised tile with a muted glyph (spec §7). */
  let {
    onClick,
    enabled,
    title = 'Send (⏎)',
    class: className,
  }: {
    onClick: () => void
    enabled: boolean
    title?: string
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
      : 'border border-line bg-raised text-muted',
    focusGold,
    className,
  )}
>
  <ArrowUp size={16} strokeWidth={2.25} />
</button>
