<script lang="ts">
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import { fade, scale, POP, QUICK } from '@/lib/motion'
  import { popPanel } from '@/components/chat/chat-chrome'
  import type { ContextMenuController, ContextMenuItem, MenuIcon } from './context-menu.svelte'

  // Renders the open menu of a `useContextMenu()` controller (this was the
  // `menu` JSX the React hook returned). Mount once per surface:
  //   <ContextMenu {menu} />
  // See context-menu.svelte.ts for the item grammar and usage.

  let { menu }: { menu: ContextMenuController } = $props()

  let el = $state<HTMLDivElement | null>(null)
  let active = $state(-1)
  let openSub = $state<number | null>(null)

  const selectable = $derived(
    (menu.state?.items ?? []).filter((i): i is ContextMenuItem => i !== 'sep' && !i.disabled),
  )

  // Rows with their selectable index precomputed (the React version ran a
  // mutable counter inline in the JSX; the template can't).
  const rows = $derived.by(() => {
    let selIdx = -1
    return (menu.state?.items ?? []).map((item, i) => {
      if (item !== 'sep' && !item.disabled) selIdx += 1
      return { item, i, idx: item !== 'sep' && !item.disabled ? selIdx : -1 }
    })
  })

  // Fresh menu, fresh keyboard/submenu state (the React version got this by
  // remounting the Menu component on every open).
  $effect(() => {
    void menu.state
    active = -1
    openSub = null
  })

  // Clamp inside the viewport once we know our size. offsetWidth/Height, not
  // getBoundingClientRect — the entrance scale would skew a rect measured
  // mid-transition.
  $effect(() => {
    const st = menu.state
    const node = el
    if (!st || !node) return
    if (st.x + node.offsetWidth > window.innerWidth - 8) node.style.left = `${Math.max(8, window.innerWidth - node.offsetWidth - 8)}px`
    if (st.y + node.offsetHeight > window.innerHeight - 8) node.style.top = `${Math.max(8, window.innerHeight - node.offsetHeight - 8)}px`
  })

  function onDocMousedown(e: MouseEvent) {
    if (!menu.state) return
    if (!el?.contains(e.target as Node)) menu.closeMenu()
  }

  function onDocKeydown(e: KeyboardEvent) {
    if (!menu.state) return
    if (e.key === 'Escape') {
      e.stopPropagation()
      menu.closeMenu()
    } else if (e.key === 'ArrowDown') {
      e.preventDefault()
      active = (active + 1) % selectable.length
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      active = (active - 1 + selectable.length) % selectable.length
    } else if (e.key === 'Enter' && active >= 0) {
      e.preventDefault()
      const it = selectable[active]
      if (it && !it.children?.length) {
        it.onSelect?.()
        menu.closeMenu()
      }
    }
  }

  function onDocScroll() {
    if (menu.state) menu.closeMenu()
  }
</script>

{#snippet menuIcon(icon: MenuIcon)}
  {#if Array.isArray(icon)}
    {@const [IconC, iconProps] = icon}
    <IconC {...iconProps} />
  {:else}
    {@render icon()}
  {/if}
{/snippet}

<svelte:document onmousedown={onDocMousedown} onkeydowncapture={onDocKeydown} onscrollcapture={onDocScroll} />

{#if menu.state}
  {@const st = menu.state}
  <div
    bind:this={el}
    use:portal
    role="menu"
    style="position: fixed; left: {st.x}px; top: {st.y}px; z-index: 80"
    class={cn(popPanel, 'min-w-44 origin-top-left')}
    oncontextmenu={(e) => e.preventDefault()}
    in:scale={POP}
    out:fade={QUICK}
  >
    {#each rows as { item, i, idx } (item === 'sep' ? `s${i}` : `${item.label}${i}`)}
      {#if item === 'sep'}
        <div class="mx-2 my-1 border-t border-line"></div>
      {:else}
        {@const hasKids = !!item.children?.length}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="relative"
          onmouseleave={() => {
            if (hasKids && openSub === i) openSub = null
          }}
        >
          <button
            type="button"
            role="menuitem"
            disabled={item.disabled}
            aria-haspopup={hasKids || undefined}
            onmouseenter={() => {
              if (item.disabled) return
              active = idx
              openSub = hasKids ? i : null
            }}
            onclick={() => {
              if (hasKids) {
                openSub = openSub === i ? null : i
                return
              }
              item.onSelect?.()
              menu.closeMenu()
            }}
            class={cn(
              'flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left font-sans text-[13px] transition-colors',
              item.disabled
                ? 'cursor-default text-muted opacity-50'
                : item.danger
                  ? cn('text-danger', active === idx && 'bg-danger/10')
                  : cn('text-fg', (active === idx || openSub === i) && 'bg-hover'),
            )}
          >
            {#if item.checked !== undefined}
              <span class="grid w-4 shrink-0 place-items-center text-accent">{item.checked ? '✓' : ''}</span>
            {:else if item.icon}
              <span class="grid w-4 shrink-0 place-items-center text-muted">{@render menuIcon(item.icon)}</span>
            {/if}
            <span class="min-w-0 flex-1 truncate">{item.label}</span>
            {#if hasKids}
              <span class="shrink-0 text-xs text-muted">▸</span>
            {/if}
          </button>
          {#if hasKids && openSub === i}
            <div class={cn(popPanel, 'absolute left-full top-0 z-10 -ml-0.5 min-w-40 origin-top-left')} in:scale={POP} out:fade={QUICK}>
              {#each item.children ?? [] as kid, k (kid === 'sep' ? `ks${k}` : `${kid.label}${k}`)}
                {#if kid === 'sep'}
                  <div class="mx-2 my-1 border-t border-line"></div>
                {:else}
                  <button
                    type="button"
                    role="menuitem"
                    disabled={kid.disabled}
                    onclick={() => {
                      kid.onSelect?.()
                      menu.closeMenu()
                    }}
                    class={cn(
                      'flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left font-sans text-[13px] transition-colors',
                      kid.disabled
                        ? 'cursor-default text-muted opacity-50'
                        : kid.danger
                          ? 'text-danger hover:bg-danger/10'
                          : 'text-fg hover:bg-hover',
                    )}
                  >
                    {#if kid.checked !== undefined}
                      <span class="grid w-4 shrink-0 place-items-center text-accent">{kid.checked ? '✓' : ''}</span>
                    {:else if kid.icon}
                      <span class="grid w-4 shrink-0 place-items-center text-muted">{@render menuIcon(kid.icon)}</span>
                    {/if}
                    <span class="min-w-0 flex-1 truncate">{kid.label}</span>
                  </button>
                {/if}
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
{/if}
