<script lang="ts" module>
  // Menus that share a group are exclusive: opening one closes the others.
  // The board filter bar is the case — Status and Assignee must not stack.
  const menuGroups = new Map<string, Set<() => void>>()

  function joinMenuGroup(group: string, close: () => void): () => void {
    let set = menuGroups.get(group)
    if (!set) {
      set = new Set()
      menuGroups.set(group, set)
    }
    set.add(close)
    return () => {
      set.delete(close)
      if (set.size === 0) menuGroups.delete(group)
    }
  }

  function closeMenuGroup(group: string, except: () => void) {
    const set = menuGroups.get(group)
    if (!set) return
    for (const close of set) if (close !== except) close()
  }
</script>

<script lang="ts">
  import { outsidePointer } from '@/lib/outside-click'
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { popPanel } from '@/components/chat/chat-chrome'
  import { DROPDOWN_PANEL_MAX_WIDTH_PX, dropdownHorizStyle } from '@/lib/dropdown-position'
  import type { ContextMenuEntry, MenuIcon } from './context-menu.svelte'

  // Anchored dropdown menu — the SAME shell and item grammar as the context
  // menu, attached to a trigger instead of the cursor. Replaces every ad-hoc
  // absolutely-positioned menu panel with the §7 popover shell.
  let {
    trigger,
    items,
    align = 'right',
    up = false,
    class: className,
    footer,
    content,
    onWillOpen,
    group,
  }: {
    /** Renders the trigger; the `open` param lets it style its active state. */
    trigger: Snippet<[boolean]>
    items: ContextMenuEntry[] | (() => ContextMenuEntry[])
    align?: 'left' | 'right'
    /** Open upward (triggers docked at the bottom of the viewport). */
    up?: boolean
    class?: string
    /** Rendered under the items (e.g. a date input) — clicks inside stay open.
     *  Receives `close`. */
    footer?: Snippet<[() => void]>
    /** Replaces the item list entirely — custom panel bodies (swatch grids,
     *  small forms). Items/footer are ignored when set. Receives `close`. */
    content?: Snippet<[() => void]>
    /** Runs before the panel opens (e.g. refresh a live list). */
    onWillOpen?: () => void | Promise<void>
    /** Opening this menu closes every other open menu in the same group. */
    group?: string
  } = $props()

  let open = $state(false)
  let ref = $state<HTMLDivElement | null>(null)
  let panelEl = $state<HTMLDivElement | null>(null)
  // Fixed-position style computed from the trigger at open time. The panel
  // PORTALS to <body>: cards/panels carry backdrop-filter (a stacking
  // context), so an absolutely-positioned sibling could never stack above
  // neighboring cards, no z-index would save it.
  let pos = $state<string | null>(null)

  const close = () => {
    open = false
  }

  $effect(() => {
    if (!group) return
    return joinMenuGroup(group, close)
  })

  async function toggle() {
    if (open) return close()
    if (group) closeMenuGroup(group, close)
    await onWillOpen?.()
    const r = ref?.getBoundingClientRect()
    if (!r) return
    const vert = up ? `bottom: ${window.innerHeight - r.top + 4}px` : `top: ${r.bottom + 4}px`
    const horiz = dropdownHorizStyle(align, r, window.innerWidth, DROPDOWN_PANEL_MAX_WIDTH_PX)
    pos = `position: fixed; z-index: 80; ${vert}; ${horiz}`
    open = true
  }

  function onDocMousedown(e: MouseEvent) {
    if (!open) return
    if (outsidePointer(e, ref, panelEl)) close()
  }

  function onDocKeydown(e: KeyboardEvent) {
    if (open && e.key === 'Escape') close()
  }

  // Reposition-on-scroll is guesswork; closing matches the context menu.
  function onDocScroll(e: Event) {
    if (!open) return
    if (panelEl?.contains(e.target as Node)) return
    close()
  }

  const entries = $derived(typeof items === 'function' ? (open ? items() : []) : items)
</script>

{#snippet menuIcon(icon: MenuIcon)}
  {#if Array.isArray(icon)}
    {@const [IconC, iconProps] = icon}
    <IconC {...iconProps} />
  {:else}
    {@render icon()}
  {/if}
{/snippet}

<svelte:document onmousedown={onDocMousedown} onkeydown={onDocKeydown} onscrollcapture={onDocScroll} />

<div bind:this={ref} class={cn('relative', className)}>
  <!-- The trigger click is the menu's — never the row/card underneath. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <span
    onclick={(e) => {
      e.stopPropagation()
      toggle()
    }}
  >
    {@render trigger(open)}
  </span>
  {#if open && pos}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <div
      bind:this={panelEl}
      use:portal
      role="menu"
      style={pos}
      onclick={(e) => e.stopPropagation()}
      class={cn(
        popPanel,
        'min-w-44 max-w-72 max-w-[min(18rem,calc(100vw-16px))]',
        up
          ? align === 'right' ? 'origin-bottom-right' : 'origin-bottom-left'
          : align === 'right' ? 'origin-top-right' : 'origin-top-left',
      )}
      in:pop={POPOVER}
      out:fade={QUICK}
    >
      {#if content}
        <div class="p-1">{@render content(close)}</div>
      {:else}
        <div class="max-h-80 overflow-y-auto">
          {#each entries as item, i (item === 'sep' ? `s${i}` : `${item.label}${i}`)}
            {#if item === 'sep'}
              <div class="mx-2 my-1 border-t border-line"></div>
            {:else}
              <button
                type="button"
                role="menuitem"
                disabled={item.disabled}
                onclick={(e) => {
                  e.stopPropagation()
                  item.onSelect?.()
                  if (!item.keepOpen) close()
                }}
                class={cn(
                  'flex w-full select-none items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left font-sans text-[13px] transition-colors',
                  item.disabled
                    ? 'cursor-default text-muted opacity-50'
                    : item.danger
                      ? 'text-danger hover:bg-danger/10'
                      : cn('dither-fill', item.checked === false ? 'text-muted' : 'text-fg'),
                )}
              >
                <!-- checked state: leading ✓ slot (kept when unchecked so rows
                    align); falls back to the icon slot otherwise. -->
                {#if item.checked !== undefined}
                  <span class="grid w-4 shrink-0 place-items-center text-accent">{item.checked ? '✓' : ''}</span>
                {/if}
                {#if item.icon}
                  <span class="grid w-4 shrink-0 place-items-center text-muted">{@render menuIcon(item.icon)}</span>
                {/if}
                <span class="min-w-0 flex-1 truncate">{item.label}</span>
              </button>
            {/if}
          {/each}
        </div>
      {/if}
      {#if !content && footer}
        <div class="mt-1 border-t border-line px-2 pb-1 pt-2">{@render footer(close)}</div>
      {/if}
    </div>
  {/if}
</div>
