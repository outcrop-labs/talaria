<script lang="ts">
  import { tick } from 'svelte'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import { useUiVoice } from '@/lib/ui-voice'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { popPanel } from '@/components/chat/chat-chrome'
  import type { ContextMenuController, ContextMenuEntry, ContextMenuItem, MenuIcon } from './context-menu.svelte'
  import { menuKeyStep, placeMenu, submenuSide } from './context-menu-keys'

  // Renders the open menu of a `useContextMenu()` controller. Mount once per
  // surface:
  //   <ContextMenu {menu} />
  // See context-menu.svelte.ts for the item grammar and the ways to open one,
  // and context-menu-keys.ts for what each key does.
  //
  // FOCUS: the panel takes focus when it opens — its first item on a keyboard
  // open, the panel itself on a pointer open — and every arrow moves real DOM
  // focus (a roving focus, so a screen reader follows it). Closing from
  // inside (Esc, Tab, choosing an item) hands focus back to whatever had it
  // before the menu opened; an outside click keeps the focus it moved.
  //
  // PLACEMENT: inside the viewport, always. A keyboard or button open sits
  // under its element and flips above when the room below is short; a
  // pointer open flips left/up off the edges; a submenu opens on whichever
  // side has room. A menu taller than the screen scrolls inside itself.

  let { menu, label = 'Actions' }: { menu: ContextMenuController; label?: string } = $props()

  const sans = useUiVoice() === 'sans'

  let el = $state<HTMLDivElement | null>(null)
  let subEl = $state<HTMLDivElement | null>(null)
  /** The active item, as an index into `selectable` (top level). */
  let active = $state(-1)
  /** The row (index into the items) whose submenu is open. */
  let openSub = $state<number | null>(null)
  /** The active submenu item, as an index into `subSelectable`. */
  let subActive = $state(-1)
  /** Keys drive the submenu rather than the top level. */
  let keysInSub = $state(false)
  let pos = $state<{ left: number; top: number } | null>(null)
  let subPos = $state<{ left: number; top: number } | null>(null)

  const isItem = (i: ContextMenuEntry): i is ContextMenuItem => i !== 'sep' && !i.disabled
  const selectable = $derived((menu.state?.items ?? []).filter(isItem))

  // Rows with their selectable index precomputed (the template can't count).
  const rows = $derived.by(() => {
    let selIdx = -1
    return (menu.state?.items ?? []).map((item, i) => {
      if (isItem(item)) selIdx += 1
      return { item, i, idx: isItem(item) ? selIdx : -1 }
    })
  })

  const subParent = $derived.by(() => {
    const it = openSub !== null ? menu.state?.items[openSub] : undefined
    return it && it !== 'sep' && it.children?.length ? it : null
  })
  const subRows = $derived.by(() => {
    let selIdx = -1
    return (subParent?.children ?? []).map((item, k) => {
      if (isItem(item)) selIdx += 1
      return { item, k, idx: isItem(item) ? selIdx : -1 }
    })
  })
  const subSelectable = $derived((subParent?.children ?? []).filter(isItem))

  const focusIn = (root: HTMLElement | null, attr: string, i: number) =>
    (root?.querySelector(`[${attr}="${i}"]`) as HTMLElement | null)?.focus({ preventScroll: true })

  // A fresh menu: fresh keyboard and submenu state, then place it (it renders
  // hidden for that one measuring tick) and give it focus.
  $effect(() => {
    const st = menu.state
    active = -1
    openSub = null
    subActive = -1
    keysInSub = false
    pos = null
    subPos = null
    if (!st) return
    void tick().then(async () => {
      const node = el
      if (!node || menu.state !== st) return
      // offsetWidth/Height, not the rect: the entrance scale would skew a rect
      // measured mid-transition.
      pos = placeMenu(
        { width: node.offsetWidth, height: node.offsetHeight },
        { width: window.innerWidth, height: window.innerHeight },
        st,
      )
      // Visible first: a hidden element cannot take focus.
      await tick()
      if (menu.state !== st) return
      if (st.keyboard && selectable.length > 0) {
        active = 0
        focusIn(node, 'data-sel', 0)
      } else {
        node.focus({ preventScroll: true })
      }
    })
  })

  /** Open the submenu of row `i` and, from the keyboard, move into it. */
  async function showSub(i: number, withKeys: boolean) {
    if (openSub !== i) {
      openSub = i
      subPos = null
      subActive = -1
    }
    keysInSub = withKeys
    await tick()
    const parentRow = el?.querySelector(`[data-row="${i}"]`)
    const sub = subEl
    if (!parentRow || !sub || openSub !== i) return
    const r = parentRow.getBoundingClientRect()
    const w = sub.offsetWidth
    const h = sub.offsetHeight
    const side = submenuSide(r, w, window.innerWidth)
    const left = side === 'right' ? r.right - 2 : r.left - w + 2
    const top = Math.max(8, Math.min(r.top - 4, window.innerHeight - 8 - h))
    subPos = { left: Math.max(8, Math.min(left, window.innerWidth - 8 - w)), top }
    await tick()
    if (withKeys && subSelectable.length > 0 && openSub === i) {
      subActive = 0
      focusIn(sub, 'data-sub-sel', 0)
    }
  }

  function hideSub() {
    openSub = null
    subPos = null
    subActive = -1
    keysInSub = false
  }

  /** Close from inside: focus goes back to where the menu came from. */
  function closeAndReturn() {
    const back = menu.state?.returnFocus ?? null
    menu.closeMenu()
    if (back?.isConnected) back.focus({ preventScroll: true })
  }

  /** Choose an item. The menu closes (and hands focus back) BEFORE the item
   *  runs, so an item that opens a dialog keeps the focus the dialog takes. */
  function choose(it: ContextMenuItem) {
    if (it.keepOpen) {
      it.onSelect?.()
      return
    }
    closeAndReturn()
    it.onSelect?.()
  }

  function onDocMousedown(e: MouseEvent) {
    if (!menu.state) return
    const t = e.target as Node
    if (!el?.contains(t) && !subEl?.contains(t)) menu.closeMenu()
  }

  function onDocKeydown(e: KeyboardEvent) {
    if (!menu.state) return
    const inSub = keysInSub && subParent !== null
    const list = inSub ? subSelectable : selectable
    const step = menuKeyStep(e.key, {
      active: inSub ? subActive : active,
      labels: list.map((i) => i.label),
      hasChildren: (i) => !inSub && !!list[i]?.children?.length,
      inSub,
    })
    if (step.kind === 'none') return
    // Tab closes and lets the browser carry focus on from where it lands.
    if (step.kind !== 'close' || e.key !== 'Tab') e.preventDefault()
    e.stopPropagation()
    switch (step.kind) {
      case 'move':
        if (inSub) {
          subActive = step.active
          focusIn(subEl, 'data-sub-sel', step.active)
        } else {
          active = step.active
          if (openSub !== null && !keysInSub) hideSub()
          focusIn(el, 'data-sel', step.active)
        }
        break
      case 'activate': {
        const it = list[step.index]
        if (it) choose(it)
        break
      }
      case 'open-sub': {
        const it = list[step.index]
        const row = rows.find((r) => r.item === it)
        if (row) void showSub(row.i, true)
        break
      }
      case 'close-sub':
        hideSub()
        focusIn(el, 'data-sel', active)
        break
      case 'close':
        closeAndReturn()
        break
    }
  }

  // Reposition-on-scroll is guesswork; closing is honest. A menu that scrolls
  // inside itself is not a page scroll.
  function onDocScroll(e: Event) {
    if (!menu.state) return
    const t = e.target as Node
    if (el?.contains(t) || subEl?.contains(t)) return
    menu.closeMenu()
  }

  const itemClass = (it: ContextMenuItem, on: boolean) =>
    cn(
      'flex w-full select-none items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left font-sans text-[13px] transition-colors focus-visible:outline-none',
      it.disabled
        ? 'cursor-default text-muted opacity-50'
        : it.danger
          ? cn('text-danger', on && 'bg-danger/10')
          : cn('text-fg', on && 'bg-hover'),
    )
</script>

{#snippet menuIcon(icon: MenuIcon)}
  {#if Array.isArray(icon)}
    {@const [IconC, iconProps] = icon}
    <IconC {...iconProps} />
  {:else}
    {@render icon()}
  {/if}
{/snippet}

{#snippet itemBody(it: ContextMenuItem, hasKids: boolean)}
  {#if it.checked !== undefined}
    <span class="grid w-4 shrink-0 place-items-center text-accent" aria-hidden="true">{it.checked ? '✓' : ''}</span>
  {:else if it.icon}
    <span class="grid w-4 shrink-0 place-items-center text-muted" aria-hidden="true">{@render menuIcon(it.icon)}</span>
  {/if}
  <span class="min-w-0 flex-1 truncate">{it.label}</span>
  {#if it.hint}
    <span class={sans ? 'shrink-0 font-sans text-xs text-muted' : 'shrink-0 font-mono text-[10px] text-ink-dim'}>{it.hint}</span>
  {/if}
  {#if hasKids}
    <span class="shrink-0 text-xs text-muted" aria-hidden="true">▸</span>
  {/if}
{/snippet}

<svelte:document onmousedown={onDocMousedown} onkeydowncapture={onDocKeydown} onscrollcapture={onDocScroll} />

{#if menu.state}
  {@const st = menu.state}
  <!-- ONE portaled root for the menu and its submenu. Svelte removes a block
       by walking its nodes from the first to the last, so a block whose
       first node was moved to <body> and whose last was not would walk on
       through <body> — and take whatever was portaled after it (a dialog an
       item just opened) with it. A single root is removed alone. -->
  <div use:portal class="contents">
    <div
      bind:this={el}
      role="menu"
      aria-label={label}
      tabindex="-1"
      data-testid="context-menu"
      style="position: fixed; left: {pos?.left ?? st.x}px; top: {pos?.top ?? st.y}px; z-index: 80; max-height: calc(100dvh - 16px); visibility: {pos ? 'visible' : 'hidden'}"
      class={cn(popPanel, 'min-w-44 origin-top-left overflow-y-auto focus:outline-none')}
      oncontextmenu={(e) => e.preventDefault()}
      in:pop={POPOVER}
      out:fade={QUICK}
    >
      {#each rows as { item, i, idx } (item === 'sep' ? `s${i}` : `${item.label}${i}`)}
        {#if item === 'sep'}
          <div role="separator" class="mx-2 my-1 border-t border-line"></div>
        {:else}
          {@const hasKids = !!item.children?.length}
          {@const on = active === idx || openSub === i}
          <button
            type="button"
            role={item.checked !== undefined ? 'menuitemcheckbox' : 'menuitem'}
            aria-checked={item.checked !== undefined ? item.checked : undefined}
            tabindex="-1"
            data-row={i}
            data-sel={idx >= 0 ? idx : undefined}
            title={item.title}
            disabled={item.disabled}
            aria-haspopup={hasKids ? 'menu' : undefined}
            aria-expanded={hasKids ? openSub === i : undefined}
            onmouseenter={(e) => {
              if (item.disabled) return
              active = idx
              keysInSub = false
              ;(e.currentTarget as HTMLElement).focus({ preventScroll: true })
              if (hasKids) void showSub(i, false)
              else if (openSub !== null) hideSub()
            }}
            onclick={(e) => {
              // A click (or a tap — which arrives as a hover, then a click)
              // opens a parent's submenu; it never toggles it shut, or a tap
              // would open and close it in one gesture.
              if (hasKids) {
                void showSub(i, e.detail === 0)
                return
              }
              choose(item)
            }}
            class={itemClass(item, on)}
          >
            {@render itemBody(item, hasKids)}
          </button>
        {/if}
      {/each}
    </div>
    {#if subParent}
      <div
        bind:this={subEl}
        role="menu"
        aria-label={subParent.label}
        tabindex="-1"
        data-testid="context-submenu"
        style="position: fixed; left: {subPos?.left ?? 0}px; top: {subPos?.top ?? 0}px; z-index: 81; max-height: calc(100dvh - 16px); visibility: {subPos ? 'visible' : 'hidden'}"
        class={cn(popPanel, 'min-w-40 origin-top-left overflow-y-auto focus:outline-none')}
        oncontextmenu={(e) => e.preventDefault()}
        in:pop={POPOVER}
        out:fade={QUICK}
      >
        {#each subRows as { item: kid, k, idx: kidx } (kid === 'sep' ? `ks${k}` : `${kid.label}${k}`)}
          {#if kid === 'sep'}
            <div role="separator" class="mx-2 my-1 border-t border-line"></div>
          {:else}
            <button
              type="button"
              role={kid.checked !== undefined ? 'menuitemcheckbox' : 'menuitem'}
              aria-checked={kid.checked !== undefined ? kid.checked : undefined}
              tabindex="-1"
              data-sub-sel={kidx >= 0 ? kidx : undefined}
              title={kid.title}
              disabled={kid.disabled}
              onmouseenter={(e) => {
                if (kid.disabled) return
                subActive = kidx
                keysInSub = true
                ;(e.currentTarget as HTMLElement).focus({ preventScroll: true })
              }}
              onclick={() => choose(kid)}
              class={itemClass(kid, keysInSub && subActive === kidx)}
            >
              {@render itemBody(kid, false)}
            </button>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
{/if}
