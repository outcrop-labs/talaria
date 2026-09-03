<script lang="ts" module>
  import type { SessionUser } from '@/lib/session'
  import { markCrossfade } from '@/lib/motion'
  import { ditherSurface } from '@/lib/dither-surface'
  import DitherLayer from '@/components/ui/DitherLayer.svelte'
  import type { DitherSource } from '@/lib/dither'

  function userInitials(user: SessionUser): string {
    const base = user.name?.trim() || user.email || '?'
    const words = base.split(/\s+/).filter(Boolean)
    const init = words.length >= 2 ? `${words[0]!.charAt(0)}${words[1]!.charAt(0)}` : base.slice(0, 2)
    return init.toUpperCase()
  }
</script>

<script lang="ts">
  import { ChevronsLeft, ChevronsRight, TriangleAlert } from '@lucide/svelte'
  import Brand from '@/components/Brand.svelte'
  import WingMark from '@/components/WingMark.svelte'
  import CreateBoardModal from '@/components/board/CreateBoardModal.svelte'
  import TeamsModal from '@/components/board/TeamsModal.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { activeAmong, isUnder } from '@/lib/route-tabs'
  import { shouldAttachInboxDecision } from '@/lib/inbox-focus-surface'
  import BoardsSublist from './BoardsSublist.svelte'
  import NavIcon from './NavIcon.svelte'
  import RailTooltip from './RailTooltip.svelte'
  import CollapsePane from '@/components/ui/CollapsePane.svelte'
  import SidebarSearch from './SidebarSearch.svelte'
  import SidebarAssistant from './SidebarAssistant.svelte'
  import { useNavCollapsed } from './nav-rail.svelte'
  import { cn } from '@/lib/cn'
  import { NAV, type NavItem } from '@/lib/nav'
  import { useEnabledApps } from '@/lib/apps'
  import { useInboxFocus, useInboxFocusSummary } from '@/lib/inbox-focus.svelte'
  import { useDeniedViews } from '@/lib/session'
  import { useUnreads } from '@/lib/unreads.svelte'
  import { route } from '@/router'

  // The main application menu. Expanded: WORK/MANAGE/SYSTEM sections with the
  // Boards sublist; collapsed: 36px icon tiles with tooltips. Active state
  // keeps the `data-status="active"` attribute the React Links carried so the
  // Tailwind `data-[status=active]` variants stay byte-identical — the match
  // itself is computed from `route.pathname` (exact vs prefix, see exactFor).
  let { user }: { user: SessionUser } = $props()

  const isAdmin = $derived(user.role === 'admin')
  const pathname = $derived(route.pathname)
  const denied = useDeniedViews()
  let creating = $state(false)
  let teamsOpen = $state(false)
  const nav = useNavCollapsed()
  // Are we ON the Inbox? The full queue loads here and only a count elsewhere.
  //
  // The SAME predicate the assistant surface uses, not a fourth spelling of it.
  // Bare `/home` renders the Inbox as its default tab, so an `isUnder(…,
  // '/home/inbox')` of its own would have quietly loaded the summary instead of
  // the queue on the URL the nav rail itself points at.
  const isInbox = $derived(shouldAttachInboxDecision(pathname, undefined))
  const inboxQueue = useInboxFocus(() => ({ enabled: isInbox }))
  const inboxSummary = useInboxFocusSummary(() => ({ enabled: !isInbox }))
  // `null` = the count could NOT be read, which is a different fact from zero
  // and must not render as one. The rail is where a person checks whether
  // anything is waiting without opening anything; a silent 0 over a failed
  // read is this app's oldest bug shape, on the one surface that gates every
  // human decision. `!` with a title is the honest badge.
  const inboxRead = $derived(isInbox ? inboxQueue : inboxSummary)
  const unread: number | null = $derived(
    inboxRead.isError && inboxRead.data === undefined
      ? null
      : isInbox
        ? (inboxQueue.data?.counts.total ?? 0)
        : (inboxSummary.data?.count ?? 0),
  )
  // The other rails' badges — Comms, Plan, Research — ride /api/unreads, the
  // same counts their pills show, live over the firehose with its own 30s
  // floor. The SAME unread-null doctrine as /home above applies, for the same
  // reason and on the same surface.
  const unreadsQuery = useUnreads()
  /** An item's badge count: undefined = carries none; null = unreadable. */
  const badgeFor = (item: NavItem): number | null | undefined => {
    if (item.to === '/home') return unread
    if (!item.badge) return undefined
    if (unreadsQuery.isError && unreadsQuery.data === undefined) return null
    return unreadsQuery.data?.[item.badge] ?? 0
  }
  const badgeTitle = (badge: number | null) =>
    badge === null ? 'Could not load what is waiting here' : undefined
  // Enabled apps get their own rail category, separate from Work: Work is
  // Talaria's own surfaces, and an app's work surface is a guest with its own
  // heading — you should be able to tell platform from app at a glance. An
  // app's MANAGE surface still slots under Manage, because Manage is the
  // control plane no matter who published the view (same grant model as any
  // core Manage view). The Apps section appears between the two and only when
  // it has something in it — no app installed, no empty heading.
  // Query handling only. `{ data: apps = [] }` discarded the query on the line
  // that made it, so a failed /api/apps silently removed every app's nav entry
  // — the surface just is not in the rail, and nothing anywhere says why.
  const appsQuery = useEnabledApps()
  const appsList = listQuery(appsQuery, { title: 'App links unavailable', variant: 'inline' })
  const appsBroken = $derived(appsList.failed || appsList.stale)
  const appWork = $derived(appsList.rows.filter((a) => a.surfaces.work).map((a) => ({ to: `/x/${a.slug}`, label: a.surfaces.work!, icon: a.icon })))
  const appManage = $derived(appsList.rows.filter((a) => a.surfaces.manage).map((a) => ({ to: `/x/${a.slug}/manage`, label: a.surfaces.manage!, icon: a.icon })))

  // Denied-view + role filtering, shared by both modes and by the app items
  // wherever they land.
  const passes = (i: NavItem) =>
    (!i.adminOnly || isAdmin) && !denied.current.includes(i.to) && !denied.current.some((d) => i.to.startsWith(d + '/'))

  const sections = $derived.by(() => {
    const core = NAV.flatMap((section) => {
      if (section.adminOnly && !isAdmin) return []
      const items = [...section.items, ...(section.title === 'Manage' ? appManage : [])].filter(passes)
      return items.length === 0 ? [] : [{ title: section.title, items }]
    })
    const apps = appWork.filter(passes)
    if (apps.length === 0) return core
    // After Work when Work exists; otherwise ahead of Manage (or at the top,
    // in the nothing-core-survives edge) — either way: Work, Apps, Manage.
    const workAt = core.findIndex((s) => s.title === 'Work')
    const at = workAt >= 0 ? workAt + 1 : Math.max(core.findIndex((s) => s.title === 'Manage'), 0)
    return [...core.slice(0, at), { title: 'Apps', items: apps }, ...core.slice(at)]
  })

  // THE ACTIVE ITEM IS THE MOST SPECIFIC ONE CONTAINING THE ROUTE, decided
  // across every section at once by `activeAmong` — so `/boards` stays lit
  // while you read a task inside it, and an app's Manage surface beats its Work
  // surface instead of lighting both.
  //
  // This replaced `exactFor`, which matched Home and app Work items EXACTLY and
  // everything else by prefix. That fixed two cases and stated no rule: Home
  // needed an exemption because it is an ancestor of every route, app Work
  // needed one because Manage nests under it, and the next nested pair would
  // have lit both again. Most-specific-wins subsumes all three.
  const activePath = $derived(activeAmong(pathname, sections.flatMap((sec) => sec.items.map((i) => i.to))))

  // AMBIENT TEXTURE ON THE RAIL, and the halo under whatever is active.
  //
  // A whisper by default and a notch louder while the pointer is over the
  // panel, so the field surfaces on approach instead of shouting all day. It
  // is presence, not interaction: nothing here is clickable and nothing about
  // it reaches assistive tech.
  //
  // Subtlety here is TRANSPARENCY, not scarcity. The field keeps a real dot
  // population and caps its alpha at a ghost of the halo's, which is what lets
  // it read as the surface having material rather than as dots scattered on
  // one. Shimmer is presence-gated and is the only motion in the rail.
  // ONE MARK FOR THE WHOLE RAIL, so the selected tile travels between items
  // instead of vanishing here and appearing there. The canvas that used to do
  // this measured the active row and drew at its coordinates, which is why it
  // lagged a row behind on scroll; an element cannot be wrong about where it
  // is.
  const [sendMark, receiveMark] = markCrossfade()

  let inside = $state(false)
  const ambient = $derived.by((): DitherSource[] => {
    const a = inside ? { chrome: 0.2, grain: 0.05, foot: 0.09 } : { chrome: 0.14, grain: 0.035, foot: 0.06 }
    return [
      { id: 'chrome', kind: 'edge', side: 'top', depth: 64, strength: a.chrome },
      { id: 'grain', kind: 'uniform', strength: a.grain },
      { id: 'foot', kind: 'edge', side: 'bottom', depth: 96, strength: a.foot },
    ]
  })

  const statusFor = (item: NavItem): 'active' | undefined => (item.to === activePath ? 'active' : undefined)
</script>

{#snippet modals()}
  <CreateBoardModal open={creating} onClose={() => (creating = false)} />
  <TeamsModal open={teamsOpen} onClose={() => (teamsOpen = false)} />
{/snippet}

<!-- CollapsePane owns the collapse/expand width glide (the two variants used
     to be separate <nav>s, so the flip snapped with no animation possible).
     Inner divs pin each variant's width so content clips during the glide
     instead of squishing. -->
<!-- `relative` anchors the two fields. CollapsePane already clips
     (`overflow-hidden`, so the width animation does not spill), which means
     the active halo's bleed is cropped at the rail's edge rather than
     spilling onto the stage. That is the right side of the trade here: a halo
     that leaks across the rail's border would read as a rendering fault. -->
<CollapsePane
  tag="nav"
  collapsed={nav.collapsed}
  width="w-[208px]"
  collapsedWidth="w-16"
  class="relative h-full shrink-0 border-r border-line bg-sidebar"
  onmouseenter={() => (inside = true)}
  onmouseleave={() => (inside = false)}
>
<DitherLayer sources={ambient} shimmer={inside ? 0.12 : 0} organic={0.5} alphaFloor={0.04} maxAlpha={0.14} />
{#if nav.collapsed}
  <!-- ── Icon rail (64px, spec §5) ───────────────────────────────────────── -->
  <div class="flex h-full w-16 flex-col items-center pb-5 pt-3">
    <div class="grid h-9 w-9 shrink-0 place-items-center" aria-label="Talaria">
      <WingMark class="h-5 w-5" />
    </div>

    <!-- Collapsed tiles get the same room, for the same reason. -->
    <!-- `px-1` is room for the selected band, not decoration. `overflow-y-auto`
         makes overflow-x compute to `auto` too, so this box clips horizontally
         even though it only asked to scroll vertically — and the outset accent
         band was being sliced off flush at both edges. -->
    <div class="mt-3 flex min-h-0 w-full flex-1 flex-col items-center gap-2 overflow-y-auto p-1">
      {#each sections as section, si (section.title)}
        {#if si > 0}<div class="my-2 h-px w-6 shrink-0 bg-line"></div>{/if}
        {#each section.items as item (item.to)}
          {@const badge = badgeFor(item)}
          <RailTooltip label={item.label}>
            <a
              href={item.to}
              data-status={statusFor(item)}
              aria-label={item.label}
              class={cn(
                'relative grid h-9 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms] dither-bloom hover:text-fg',
                'data-[status=active]:bg-raised data-[status=active]:text-fg',
                '[&[data-status=active]_svg]:h-[22px] [&[data-status=active]_svg]:w-[22px]',
              )}
            >
              <NavIcon icon={item.icon} />
              {#if badge !== undefined && (badge === null || badge > 0)}
                <span
                  title={badgeTitle(badge)}
                  class={cn(
                    'absolute right-0.5 top-0 font-mono text-[10px] leading-3 tracking-[0.05em]',
                    badge === null ? 'text-[color:var(--theme-danger)]' : 'text-muted',
                  )}
                >
                  {badge === null ? '!' : badge}
                </span>
              {/if}
            </a>
          </RailTooltip>
        {/each}
      {/each}
    </div>

    <div class="mt-3 flex shrink-0 flex-col items-center gap-3">
      <!-- Same launcher, icon-sized. The assistant panel no longer keeps a
           strip of its own, so without this it would be unreachable in the
           collapsed rail. -->
      <SidebarAssistant collapsed />
      <!-- Collapsed there is no room for the notice itself, but a rail that
           quietly drops every app icon is the original incident in
           miniature: the surface is simply gone and nothing says why. The
           marker says the list is missing; clicking asks again. -->
      {#if appsBroken}
        <RailTooltip label="App links unavailable; retry">
          <button
            type="button"
            onclick={() => void appsQuery.refetch()}
            aria-label="App links unavailable; retry"
            class="grid h-9 w-9 place-items-center rounded-md text-danger transition-colors duration-[120ms] dither-fill"
          >
            <TriangleAlert size={16} strokeWidth={1.5} />
          </button>
        </RailTooltip>
      {/if}
      <RailTooltip label="Expand">
        <button
          type="button"
          onclick={nav.toggleCollapsed}
          aria-label="Expand navigation"
          class="grid h-9 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
        >
          <ChevronsRight size={16} strokeWidth={1.5} />
        </button>
      </RailTooltip>
      <RailTooltip label={user.name ?? user.email ?? 'Account'}>
        <div class="grid h-[26px] w-[26px] place-items-center rounded-full border border-line-strong bg-raised font-mono text-[10px] font-medium tracking-[0.05em] text-muted">
          {userInitials(user)}
        </div>
      </RailTooltip>
    </div>

    {@render modals()}
  </div>
{:else}
  <!-- ── Sidebar (208px, spec §5) ──────────────────────────────────────────── -->
  <div class="flex h-full w-[208px] flex-col px-3 pb-4 pt-5">
    <div class="flex h-6 shrink-0 items-center">
      <Brand />
    </div>

    <SidebarSearch />
    <!-- The assistant sits above the menu, in the space the project/task meters
         used to hold: it is the one thing here that is *yours* rather than a
         place to go, and it is what the removed 44px strip beside the nav used
         to advertise. -->
    <SidebarAssistant />
    <div class="mt-4 h-px shrink-0 bg-line-subtle"></div>

    <!-- Room for the selected band at both edges — see the note above. -->
    <div class="mt-3 flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-1">
      {#each sections as section (section.title)}
        <div>
          <div class="flex h-6 items-center px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            {section.title}
          </div>
          <!-- `space-y-1.5` rather than `space-y-px`: the row treatment blooms
               OUTWARD — the hover texture fades past the row's edge and the
               active halo reaches further still — so rows a single pixel apart
               had their fields touching, which reads as one continuous band
               instead of a marked row. 6px is the smallest gap at which each
               row's field resolves as its own. -->
          <ul class="space-y-1.5">
            {#each section.items as item (item.to)}
              {@const badge = badgeFor(item)}
              <li>
                <a
                  {@attach ditherSurface()}
                  href={item.to}
                  data-status={statusFor(item)}
                  class={cn(
                    'flex h-[30px] items-center gap-[9px] rounded-md px-2 font-sans text-[13px] leading-4 text-muted transition-colors duration-[120ms] hover:text-fg',
                    // The selected tile is the MARK below, not a background on
                    // this row — that is what lets it slide between rows.
                    'relative data-[status=active]:font-medium data-[status=active]:text-fg',
                    '[&[data-status=active]_.nav-bar]:bg-accent [&[data-status=active]_.nav-ico]:text-fg',
                  )}
                >
                  {#if statusFor(item)}
                    <span
                      aria-hidden="true"
                      in:receiveMark={{ key: 'rail-mark' }}
                      out:sendMark={{ key: 'rail-mark' }}
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
                    <!-- Spec §5: nav counts are muted (#8E877E) — not accent.
                         The unreadable case is the one exception: it is not a
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
                {#if item.to === '/boards' && isUnder(pathname, '/boards')}
                  <BoardsSublist activePath={pathname} onNew={() => (creating = true)} onTeams={() => (teamsOpen = true)} />
                {/if}
              </li>
            {/each}
          </ul>
        </div>
      {/each}
    </div>

    <!-- The apps read decides which entries exist above. Dropping its failure
         here is how a surface disappears from the rail with nothing to say so
         — render the notice the query handed us. -->
    {#if appsList.notice}
      <div class="mt-3 shrink-0 px-1"><QueryError {...appsList.notice} /></div>
    {/if}

    <!-- "Local · Operator" used to sit here with a green dot: a deployment mode
         and a role, stated as though they were status, next to an indicator
         that was hard-coded green and so reported nothing. It answered a
         question nobody asks mid-work and it was never once actionable. -->
    <div class="mt-4 flex h-5 shrink-0 items-center gap-2 px-1">
      <button
        type="button"
        onclick={nav.toggleCollapsed}
        aria-label="Collapse navigation"
        class="ml-auto grid h-5 w-5 shrink-0 place-items-center rounded text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
      >
        <ChevronsLeft size={13} strokeWidth={1.5} />
      </button>
    </div>

    {@render modals()}
  </div>
{/if}
</CollapsePane>
