<script lang="ts">
  import { Pin, PinOff } from '@lucide/svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import NavIcon from './NavIcon.svelte'
  import { fly, markCrossfade, PANEL_X } from '@/lib/motion'
  import { ditherSurface } from '@/lib/dither-surface'
  import { outsidePointer } from '@/lib/outside-click'
  import { badgeTitle, useNavBadges } from '@/lib/nav-badges.svelte'
  import { appManageItem, navActiveItem, navSections } from '@/lib/nav-sections'
  import { listQuery } from '@/components/ui/query-state'
  import { useEnabledApps } from '@/lib/apps'
  import { useDeniedViews, type SessionUser } from '@/lib/session'
  import { NAV, type NavItem } from '@/lib/nav'
  import { useManageSidebar, closeManageSidebar } from './nav-dock.svelte'
  import { cn } from '@/lib/cn'
  import { route } from '@/router'

  // THE MANAGE SIDEBAR — the control plane the dock's gear opens, the piece
  // of the old rail that was never a view switch. Everything here is the
  // expanded rail's own grammar on purpose (208px, section header, 30px rows
  // with the nav-bar accent): a person who knew the rail should recognize
  // the manage half of it at a glance, and the row classes are the same ones
  // so the mark and the bloom behave identically.
  //
  // NOT A POPOVER. The ticket calls for a sidebar, and the four decisions a
  // popover shell owns would each answer it wrong: outside-click must close
  // it, but the dock's gear tile is outside it BY DESIGN (a second click
  // re-toggles, it must not close-then-reopen), and it must never be
  // portaled to <body> — the pane lives below the dock band, inside the
  // same relative row, so its geometry is decided once by the layout and
  // stays true on every resize and DesktopTitlebar permutation. The
  // census-safe shape follows from that: no portal, no fixed positioning —
  // an absolutely-positioned pane in a relative parent, with its own
  // document-level mousedown (via svelte:document, exempting the tile) and
  // Escape. The popover census scans for portal/fixed panels with document
  // listeners; this file holds the listener but no panel of that kind, so
  // the conjunction cannot fire. It is a SIDEBAR.
  //
  // `user` is the viewer whose grants decide the rows — the same value the
  // dock derives from, handed down rather than re-read, so the two cannot
  // drift.
  let { user }: { user: SessionUser } = $props()

  const denied = useDeniedViews()
  const badges = useNavBadges(() => route.pathname)
  // THE OPEN STATE IS READ THROUGH THE OBJECT, NEVER DESTRUCTURED.
  // `useManageSidebar()` hands back a GETTER over module `$state`; the
  // `const { manageOpen } = ...` this replaces evaluated that getter once, at
  // component init, and froze the answer at `false` for the component's life —
  // which is exactly how this pane shipped unable to open at all. The gear lit
  // (TopDock holds the object, so it stayed live) and nothing appeared. Keep
  // the object; read `manage.manageOpen` at every use.
  const manage = useManageSidebar()

  // The apps read, same doctrine as the rail: keep the query, default off
  // it, surface the failure. App manage surfaces slot into the manage
  // section by section id, exactly as they did under the rail.
  const appsQuery = useEnabledApps()
  const appsList = listQuery(appsQuery, { title: 'App links unavailable', variant: 'inline' })

  const appManage = $derived(
    appsList.rows.flatMap((a) => {
      const item = appManageItem(a)
      return item === null ? [] : [item]
    }),
  )

  // THE SAME DERIVATION as the dock — one call, manage section kept, work
  // and apps discarded — so the sidebar and the dock can never disagree
  // about what the manage half contains. Core manage items ride NAV; the
  // gateable list is the cross-check the grants surface uses.
  const sections = $derived(
    navSections(NAV, [], appManage, { isAdmin: user.role === 'admin', denied: denied.current }),
  )
  const manageSection = $derived(sections.find((s) => s.id === 'manage') ?? null)

  // Empty-manage edge: a member with zero visible manage items gets nothing.
  // useDeniedViews already drives AppLayout's route gate (a denied or
  // role-gated view reached by URL bounces to Home); rendering the gear or
  // the pane with no rows would be an affordance to an empty room.
  const isEmpty = $derived(manageSection === null || manageSection.items.length === 0)

  // Active state via the same most-specific-wins rule, over the manage
  // section alone — the pane is only ever about these paths.
  const activePath = $derived(manageSection ? (navActiveItem(route.pathname, [manageSection])?.to ?? null) : null)

  // ONE MARK FOR THE WHOLE PANE, the rail's rule restated: the selected tile
  // travels between rows rather than vanishing here and appearing there.
  const [sendMark, receiveMark] = markCrossfade()

  // Outside-click: a mousedown anywhere but the pane, the gear tile, or a
  // row inside the pane closes it. The tile carries `data-manage-tile` (the
  // dock's own toggle must not be double-handled here).
  let paneEl = $state<HTMLElement | null>(null)
  function onDocMousedown(e: MouseEvent) {
    if (!manage.manageOpen || isEmpty) return
    const tile = (e.target as HTMLElement | null)?.closest('[data-manage-tile]')
    if (tile) return
    if (outsidePointer(e, paneEl, paneEl)) closeManageSidebar()
  }
  function onDocKeydown(e: KeyboardEvent) {
    if (manage.manageOpen && e.key === 'Escape') closeManageSidebar()
  }

  // Pin: keep the pane open while walking between manage views. A second
  // click unpins; closing the pane clears the pin, so a reopen is fresh.
  let pinned = $state(false)
  function onPin() {
    if (!pinned) pinned = true
    else {
      pinned = false
      closeManageSidebar()
    }
  }

  // Navigating to a manage view IS the choice made — close behind it, so the
  // pane is a door, not a resident overlay. Three things about this effect,
  // each of which it got wrong before:
  //
  //  1. IT IS ABOUT A CHANGE OF ROUTE, NOT ABOUT WHERE YOU ALREADY ARE. The
  //     old version asked only "is the current path a manage view?", which is
  //     true the whole time you are standing on one — so opening the pane
  //     from /models closed it again in the same flush, and the gear was dead
  //     on every one of the eight views the pane exists to reach. `seenPath`
  //     is a PLAIN let, not `$state`, on purpose: it is this effect's own
  //     memory of the last path it saw, and writing it must not re-run the
  //     effect that wrote it.
  //  2. PINNED MEANS PINNED. The pin is for walking between manage views, and
  //     the old effect closed on every manage navigation regardless, so the
  //     pinned state only ever changed its own icon.
  //  3. THE TEST IS "the pane has a row for where you just went" —
  //     `activePath`, the same derivation the rows paint with — not a scan of
  //     MANAGE_VIEWS, which lists the eight core views and misses every app
  //     manage surface the pane also carries.
  //
  // Navigating AWAY to a work view is not this effect's business: a click on
  // a dock tile is a pointer outside the pane, and `onDocMousedown` closes it.
  let seenPath = route.pathname
  $effect(() => {
    const at = route.pathname
    const open = manage.manageOpen
    const landedOnARow = activePath !== null
    const moved = at !== seenPath
    seenPath = at
    if (!open || pinned || !moved) return
    if (landedOnARow) closeManageSidebar()
  })

  // A close clears the pin — stated in the comment above since the pane
  // shipped, and nothing did it: an outside-click on a pinned pane left
  // `pinned` true, so the next open came back pinned and the pin button's
  // first click UNpinned. One line, so a reopen is genuinely fresh.
  $effect(() => {
    if (!manage.manageOpen) pinned = false
  })

  const statusFor = (item: NavItem): 'active' | undefined => (item.to === activePath ? 'active' : undefined)
</script>

<svelte:document onmousedown={onDocMousedown} onkeydown={onDocKeydown} />

{#if manage.manageOpen && !isEmpty}
  <!-- z-[45]: above the dock's z-40 band, below modals (z-50) and portaled
       popovers (z-[60]) — the pane is navigation chrome, and a dialog opened
       from a manage view must sit above it. `inset-y-0 left-0` spans the
       below-dock row (the pane's parent is the relative row that holds the
       page), so it covers the view's full height — matching the rail's
       footprint, which also stood beside the whole page. -->
  <aside
    bind:this={paneEl}
    transition:fly={{ ...PANEL_X, x: -20 }}
    class="absolute inset-y-0 left-0 z-[45] flex w-[208px] shrink-0 flex-col border-r border-line bg-sidebar shadow-[var(--theme-shadow-3)]"
    aria-label="Manage"
  >
    <div class="flex h-9 shrink-0 items-center justify-between pl-2 pr-1.5">
      <!-- The section header, the rail's own voice: 10px mono uppercase. -->
      <div class="flex h-6 items-center px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Manage
      </div>
      <IconButton size="sm" title={pinned ? 'Unpin — close on navigation' : 'Pin open while browsing'} onclick={onPin} active={pinned}>
        {#if pinned}<Pin size={13} strokeWidth={1.5} />{:else}<PinOff size={13} strokeWidth={1.5} />{/if}
      </IconButton>
    </div>

    <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-1">
      {#if manageSection}
        <!-- `space-y-1.5` rather than `space-y-px`: the row treatment blooms
             OUTWARD — the rail's lesson, carried verbatim. -->
        <ul class="space-y-1.5">
          {#each manageSection.items as item (item.to)}
            {@const badge = badges.badgeFor(item)}
            <li>
              <a
                {@attach ditherSurface()}
                href={item.to}
                data-status={statusFor(item)}
                class={cn(
                  'flex h-[30px] items-center gap-[9px] rounded-md px-2 font-sans text-[13px] leading-4 text-muted transition-colors duration-[120ms] hover:text-fg',
                  'relative data-[status=active]:font-medium data-[status=active]:text-fg',
                  '[&[data-status=active]_.nav-bar]:bg-accent [&[data-status=active]_.nav-ico]:text-fg',
                )}
              >
                {#if statusFor(item)}
                  <span
                    aria-hidden="true"
                    in:receiveMark={{ key: 'manage-mark' }}
                    out:sendMark={{ key: 'manage-mark' }}
                    class="absolute inset-0 rounded-md bg-raised"
                    {@attach ditherSurface({ band: 6, always: () => true, selected: () => true })}
                  ></span>
                {/if}
                <span class="nav-bar relative h-3.5 w-[3px] shrink-0 rounded-[2px] bg-transparent" aria-hidden="true"></span>
                <span class="nav-ico relative grid h-4 w-4 shrink-0 place-items-center text-muted">
                  <NavIcon icon={item.icon} />
                </span>
                <span class="relative flex-1 truncate">{item.label}</span>
                {#if badge !== undefined && (badge === null || badge > 0)}
                  <!-- Spec §5: nav counts are muted — not accent. The
                       unreadable case is the one exception: it is not a
                       count, and a muted "!" reads as decoration. -->
                  <span
                    title={badgeTitle(badge)}
                    class={cn(
                      'relative font-mono text-[10px] leading-3 tracking-[0.05em]',
                      badge === null ? 'text-[color:var(--theme-danger)]' : 'text-muted',
                    )}
                  >
                    {badge === null ? '!' : badge}
                  </span>
                {/if}
              </a>
            </li>
          {/each}
        </ul>
      {/if}

      <!-- The apps read decides which app entries exist above. Dropping its
           failure here is how a surface disappears with nothing to say so —
           render the notice the query handed us, same doctrine as the rail. -->
      {#if appsList.notice}
        <div class="shrink-0 px-1"><QueryError {...appsList.notice} /></div>
      {/if}
    </div>
  </aside>
{/if}