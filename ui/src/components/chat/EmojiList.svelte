<script lang="ts">
  import { cn } from '@/lib/cn'
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
    active = 0 // new result set → selection back to the top
  }

  export function onKeyDown(e: KeyboardEvent): boolean {
    if (e.key === 'ArrowDown') {
      active = (active + 1) % Math.max(items.length, 1)
      return true
    }
    if (e.key === 'ArrowUp') {
      active = (active - 1 + items.length) % Math.max(items.length, 1)
      return true
    }
    if (e.key === 'Enter' || e.key === 'Tab') {
      if (items[active]) command(items[active])
      return true
    }
    return false
  }
</script>

{#if items.length > 0}
  <div class={cn(popPanel, 'w-56')}>
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
