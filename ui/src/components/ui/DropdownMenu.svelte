<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { popPanel } from '@/components/chat/chat-chrome'
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

  function toggle() {
    if (open) return close()
    const r = ref?.getBoundingClientRect()
    if (!r) return
    const vert = up ? `bottom: ${window.innerHeight - r.top + 4}px` : `top: ${r.bottom + 4}px`
    const horiz = align === 'right' ? `right: ${Math.max(8, window.innerWidth - r.right)}px` : `left: ${Math.max(8, r.left)}px`
    pos = `position: fixed; z-index: 80; ${vert}; ${horiz}`
    open = true
  }

  function onDocMousedown(e: MouseEvent) {
    if (!open) return
    const t = e.target as Node
    if (!ref?.contains(t) && !panelEl?.contains(t)) close()
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
        'min-w-44 max-w-72',
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
                  'flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left font-sans text-[13px] transition-colors',
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
