<script lang="ts">
  // The composer's stop affordance (Mercury, spec §7): a 36×36 rail tile that
  // pulses while a reply streams. (Send, its counterpart, lives in
  // SendButton.svelte.)
  import { Square } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'

  /** Stop pulses while a reply streams — motion marks the live, interruptible
   *  moment. Esc triggers it from the keyboard. */
  let { onClick, title = 'Stop (Esc)' }: { onClick: () => void; title?: string } = $props()
</script>

<button data-dither-fill
  type="button"
  {title}
  aria-label={title}
  onclick={onClick}
  class={cn(
    'relative grid h-9 w-9 shrink-0 place-items-center rounded-md border border-line-strong text-muted transition-colors hover:text-fg',
    focusGold,
  )}
>
  <span class="gd-pulse absolute inset-0 rounded-md border border-[color:var(--theme-accent-border)]"></span>
  <Square size={12} fill="currentColor" class="gd-pulse" />
</button>
