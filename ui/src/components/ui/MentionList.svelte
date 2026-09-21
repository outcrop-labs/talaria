<script lang="ts">
  import { createListNav } from '@/lib/list-nav'
  import { cn } from '@/lib/cn'
  import { fade, listStagger, pop, POPOVER, QUICK } from '@/lib/motion'
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
    nav.reset() // new result set → selection back to the top
  }

  const nav = createListNav<Mentionable>({
    items: () => items,
    active: () => active,
    setActive: (i) => (active = i),
    choose: (item) => command(item),
  })

  export const onKeyDown = (e: KeyboardEvent): boolean => nav.onKeyDown(e)

  // Keep the active row in view while arrowing through the list.
  $effect(() => {
    void active
    listEl?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' })
  })
</script>

{#if items.length > 0}
  <div
    bind:this={listEl}
    in:pop={POPOVER}
    out:fade={QUICK}
    use:listStagger
    class={cn(popPanel, 'max-h-56 w-60 origin-top-left overflow-y-auto')}
  >
    {#each items as item, i (item.insert)}
      <button
        type="button"
        data-active={i === active}
        onmouseenter={() => (active = i)}
        onmousedown={(e) => {
          e.preventDefault()
          command(item)
        }}
        class={cn('flex w-full select-none items-baseline gap-2 rounded-md px-2 py-1.5 text-left transition-colors', i === active ? 'bg-hover' : 'dither-fill')}
      >
        <span class="truncate font-sans text-[13px] text-fg">{item.label}</span>
        {#if item.sub}
          <span class="ml-auto truncate font-mono text-[10px] tracking-[0.05em] text-muted">{item.sub}</span>
        {/if}
      </button>
    {/each}
  </div>
{/if}
