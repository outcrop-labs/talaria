<script lang="ts">
  import { cn } from '@/lib/cn'
  import { popPanel } from '@/components/chat/chat-chrome'
  import type { Mentionable } from '@/components/chat/mentions.svelte'

  // The "@" mention menu — mounted imperatively (svelte `mount`) by
  // mention-suggest.ts: tiptap's Suggestion utility drives it from outside any
  // component tree, so fresh items/command arrive via the exported `update`
  // instead of props.

  let items = $state<Mentionable[]>([])
  let command = $state<(item: Mentionable) => void>(() => {})
  let active = $state(0)
  let listEl = $state<HTMLDivElement | null>(null)

  export function update(next: Mentionable[], cmd: (item: Mentionable) => void) {
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

  // Keep the active row in view while arrowing through the list.
  $effect(() => {
    void active
    listEl?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' })
  })
</script>

{#if items.length > 0}
  <div bind:this={listEl} class={cn(popPanel, 'max-h-56 w-60 overflow-y-auto')}>
    {#each items as item, i (item.insert)}
      <button
        type="button"
        data-active={i === active}
        onmouseenter={() => (active = i)}
        onmousedown={(e) => {
          e.preventDefault()
          command(item)
        }}
        class={cn('flex w-full items-baseline gap-2 rounded-md px-2 py-1.5 text-left transition-colors', i === active ? 'bg-hover' : 'hover:bg-hover')}
      >
        <span class="truncate font-sans text-[13px] text-fg">{item.label}</span>
        {#if item.sub}
          <span class="ml-auto truncate font-mono text-[10px] tracking-[0.05em] text-muted">{item.sub}</span>
        {/if}
      </button>
    {/each}
  </div>
{/if}
