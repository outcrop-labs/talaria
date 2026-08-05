<script lang="ts">
  import { cn } from '@/lib/cn'
  import { fade, listStagger, pop, POPOVER, QUICK } from '@/lib/motion'
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

<!-- |global: the panel IS the component root — hosts render it {#if}-gated,
     so local legs never play (ANIMATIONS.md, the |global rule). -->
<div in:pop|global={POPOVER} out:fade|global={QUICK} class={cn(popPanel, 'z-10 w-56 overflow-hidden', className)} use:listStagger>
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
