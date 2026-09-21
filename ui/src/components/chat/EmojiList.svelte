<script lang="ts">
  import { createListNav } from '@/lib/list-nav'
  import { cn } from '@/lib/cn'
  import { fade, listStagger, pop, POPOVER, QUICK } from '@/lib/motion'
  import { popPanel, popRow } from '@/components/chat/chat-chrome'
  import type { EmojiEntry } from '@/lib/emoji'

  // The ":" emoji menu — mounted imperatively (svelte `mount`) by
  // emoji-suggest.ts: tiptap's Suggestion utility drives it from outside any
  // component tree, so fresh items/command arrive via the exported `update`
  // instead of props.

  let items = $state<EmojiEntry[]>([])
  let command = $state<(item: EmojiEntry) => void>(() => {})
  let active = $state(0)

  export function update(next: EmojiEntry[], cmd: (item: EmojiEntry) => void) {
    items = next
    command = cmd
    nav.reset() // new result set → selection back to the top
  }

  const nav = createListNav<EmojiEntry>({
    items: () => items,
    active: () => active,
    setActive: (i) => (active = i),
    choose: (item) => command(item),
  })

  export const onKeyDown = (e: KeyboardEvent): boolean => nav.onKeyDown(e)
</script>

{#if items.length > 0}
  <div in:pop={POPOVER} out:fade={QUICK} class={cn(popPanel, 'w-56')} use:listStagger>
    {#each items as item, i (item.ch)}
      <button
        type="button"
        onmouseenter={() => (active = i)}
        onmousedown={(e) => {
          e.preventDefault()
          command(item)
        }}
        class={cn(popRow, i === active && 'bg-hover')}
      >
        <span class="text-base">{item.ch}</span>
        <span class="truncate font-mono text-[11px] tracking-[0.02em] text-muted">:{item.names[0]}:</span>
      </button>
    {/each}
  </div>
{/if}
