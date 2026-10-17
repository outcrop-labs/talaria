<script lang="ts">
  import { createListNav } from '@/lib/list-nav'
  import { cn } from '@/lib/cn'
  import { fade, listStagger, pop, POPOVER, QUICK } from '@/lib/motion'
  import { popPanel } from '@/components/chat/chat-chrome'
  import { Plus, X } from '@lucide/svelte'
  import type { CommandItem } from './command-suggest'

  // The "/" command menu — mounted imperatively (svelte `mount`) by the
  // Suggestion extension in ChatComposer.svelte, exactly as MentionList and
  // EmojiList are: tiptap drives it from outside any component tree, so fresh
  // items and handlers arrive through the exported `update` instead of props.
  //
  // Three row species, one list, because one list is what the keyboard walks:
  // a SKILL (inserts its token), a BLUEPRINT (its body replaces the draft),
  // and the menu's own last row, "Save as blueprint". The rules that decide
  // which rows exist and in what order live in ./command-suggest.ts.

  let items = $state<CommandItem[]>([])
  let command = $state<(item: CommandItem) => void>(() => {})
  let remove = $state<(item: CommandItem) => void>(() => {})
  let active = $state(0)
  let listEl = $state<HTMLDivElement | null>(null)

  export function update(
    next: CommandItem[],
    cmd: (item: CommandItem) => void,
    rm: (item: CommandItem) => void,
  ) {
    items = next
    command = cmd
    remove = rm
    nav.reset() // new result set → selection back to the top
  }

  const nav = createListNav<CommandItem>({
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

  const rowClass = (isActive: boolean) =>
    cn(
      'flex w-full select-none items-baseline gap-2 rounded-md px-2 py-1.5 text-left transition-colors',
      isActive ? 'bg-hover' : 'dither-fill',
    )
</script>

{#if items.length > 0}
  <div
    bind:this={listEl}
    in:pop={POPOVER}
    out:fade={QUICK}
    use:listStagger
    class={cn(popPanel, 'max-h-64 w-80 origin-top-left overflow-y-auto')}
  >
    {#each items as item, i (`${item.kind}:${item.key}`)}
      {#if item.kind === 'save'}
        <!-- The menu's own action, below a hairline: everything above inserts
             something, this one keeps what is already written. -->
        <div class="mt-1 border-t border-line pt-1">
          <button
            type="button"
            data-active={i === active}
            onmouseenter={() => (active = i)}
            onmousedown={(e) => {
              e.preventDefault()
              command(item)
            }}
            class={rowClass(i === active)}
          >
            <Plus size={12} class="shrink-0 self-center text-muted" />
            <span class="truncate font-sans text-[13px] text-fg">{item.label}</span>
            <span class="ml-auto shrink-0 font-sans text-[11px] text-muted">{item.detail}</span>
          </button>
        </div>
      {:else}
        <!-- `relative group/row` so a blueprint's delete can sit over the row
             without nesting a button inside one. -->
        <div class="relative group/row">
          <button
            type="button"
            data-active={i === active}
            onmouseenter={() => (active = i)}
            onmousedown={(e) => {
              e.preventDefault()
              command(item)
            }}
            class={rowClass(i === active)}
          >
            {#if item.kind === 'skill'}
              <span class="shrink-0 font-mono text-[12px] text-fg">/{item.label}</span>
            {:else}
              <span class="truncate font-sans text-[13px] text-fg">{item.label}</span>
              <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">blueprint</span>
            {/if}
            {#if item.detail}
              <span class={cn('ml-auto truncate font-sans text-[11px] text-muted', item.canDelete && 'group-hover/row:pr-5 group-focus-within/row:pr-5')}>
                {item.detail}
              </span>
            {/if}
          </button>
          {#if item.kind === 'blueprint' && item.canDelete}
            <div class="absolute inset-y-0 right-1 hidden items-center group-focus-within/row:flex group-hover/row:flex">
              <button
                type="button"
                title={`Delete blueprint "${item.label}"`}
                aria-label={`Delete blueprint "${item.label}"`}
                onmousedown={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  remove(item)
                }}
                class="grid h-5 w-5 place-items-center rounded text-muted transition-colors hover:text-danger focus-visible:text-danger focus-visible:outline-none"
              >
                <X size={11} />
              </button>
            </div>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
{/if}
