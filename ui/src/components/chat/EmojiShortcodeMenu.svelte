<script lang="ts">
  import { cn } from '@/lib/cn'
  import { popPanel, popRow } from '@/components/chat/chat-chrome'
  import type { EmojiShortcodeState } from './emoji.svelte'

  // The :shortcode: autocomplete menu — driven by useEmojiShortcodes (see
  // emoji.svelte.ts). Position it with `class` (host-specific).
  let {
    state,
    picked,
    onPick,
    class: className,
  }: {
    state: EmojiShortcodeState
    picked: number
    onPick: (ch: string) => void
    class?: string
  } = $props()
</script>

<div class={cn(popPanel, 'z-10 w-56 overflow-hidden', className)}>
  {#each state.options as e, i (e.ch)}
    <button
      type="button"
      onmousedown={(ev) => {
        ev.preventDefault()
        onPick(e.ch)
      }}
      class={cn(popRow, i === picked ? 'bg-hover text-fg' : 'text-muted')}
    >
      <span class="text-base">{e.ch}</span>
      <span class="truncate font-mono text-[11px] tracking-[0.02em]">:{e.names[0]}:</span>
    </button>
  {/each}
</div>
