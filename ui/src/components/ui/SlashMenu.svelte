<script lang="ts">
  import { cn } from '@/lib/cn'
  import { popPanel } from '@/components/chat/chat-chrome'
  import type { SlashItem } from './slash-commands'

  // The "/" block menu — mounted imperatively (svelte `mount`) by
  // slash-commands.ts: tiptap's Suggestion utility drives it from outside any
  // component tree, so fresh items/command arrive via the exported `update`
  // instead of props.

  let items = $state<SlashItem[]>([])
  let command = $state<(item: SlashItem) => void>(() => {})
  let active = $state(0)
  let listEl = $state<HTMLDivElement | null>(null)

  export function update(next: SlashItem[], cmd: (item: SlashItem) => void) {
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
    if (e.key === 'Enter') {
      const item = items[active]
      if (item) command(item)
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

{#if items.length === 0}
  <div class={cn(popPanel, 'w-64 p-3 font-sans text-xs text-muted')}>No blocks match.</div>
{:else}
  <div bind:this={listEl} class={cn(popPanel, 'max-h-72 w-64 overflow-y-auto')}>
    {#each items as item, i (item.title)}
      {@const Icon = item.icon}
      <button
        type="button"
        data-active={i === active}
        onmouseenter={() => (active = i)}
        onmousedown={(e) => {
          e.preventDefault()
          command(item)
        }}
        class={cn('flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left transition-colors', i === active ? 'bg-hover' : 'hover:bg-hover')}
      >
        <span class={cn('grid h-7 w-7 shrink-0 place-items-center rounded-md border border-line bg-raised', i === active ? 'text-accent' : 'text-muted')}>
          <Icon size={15} />
        </span>
        <span class="min-w-0">
          <span class="block truncate font-sans text-[13px] font-medium text-fg">{item.title}</span>
          <span class="block truncate font-sans text-[11px] text-muted">{item.hint}</span>
        </span>
      </button>
    {/each}
  </div>
{/if}
