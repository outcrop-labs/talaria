<script lang="ts" module>
  import type { SessionUser } from '@/lib/session'
  import { navSections, navActiveItem, appWorkItem, appManageItem } from '@/lib/nav-sections'
  import { NAV } from '@/lib/nav'

  // The section derivation the rail and the sidebar share, wired for the
  // dock's narrower question: which items are tiles at all? Manage is not —
  // the core manage views live in the ManageSidebar the gear opens, so the
  // dock's row is Work, then Apps. App manage surfaces live in the sidebar
  // with the core ones, by the same section-id slotting as the rail.
  function dockSections(
    user: SessionUser,
    appsRows: { slug: string; icon: string; surfaces: { work?: string; manage?: string } }[],
    denied: readonly string[],
  ) {
    const appWork = appsRows.flatMap((a) => {
      const item = appWorkItem(a)
      return item === null ? [] : [item]
    })
    const appManage = appsRows.flatMap((a) => {
      const item = appManageItem(a)
      return item === null ? [] : [item]
    })
    // Core minus manage, then the manage items through the same derivation:
    // navSections appends app manage entries to the section whose id is
    // 'manage', which we keep in the result so the SIDEBAR and the dock see
    // one derivation, not two.
    const sections = navSections(NAV, appWork, appManage, { isAdmin: user.role === 'admin', denied })
    return {
      row: sections.filter((s) => s.id !== 'manage'),
      manage: sections.find((s) => s.id === 'manage') ?? null,
    }
  }
</script>

<script lang="ts">
  import { Cog, TriangleAlert } from '@lucide/svelte'
  import WingMark from '@/components/WingMark.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import NavIcon from './NavIcon.svelte'
  import RailTooltip from './RailTooltip.svelte'
  import DesktopSwitcher from './DesktopSwitcher.svelte'
  import SidebarAssistant from './SidebarAssistant.svelte'
  import { badgeTitle, useNavBadges } from '@/lib/nav-badges.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import { useDeniedViews } from '@/lib/session'
  import { toggleManageSidebar, useManageSidebar } from './nav-dock.svelte'
  import { cn } from '@/lib/cn'
  import { route } from '@/router'

  // THE DOCK, the shell's primary nav: a full-width band of icon tiles above
  // everything, replacing the rail as the place a person moves through views.
  // Same tiles as the rail's collapsed mode — h-9 w-9, data-status=active,
  // RailTooltip, the badge doctrine — laid out horizontally instead of in a
  // column. The manage section does not get tiles: it lives behind the gear,
  // in the ManageSidebar, exactly where the rail kept it last.
  let { user }: { user: SessionUser } = $props()

  const denied = useDeniedViews()
  const badges = useNavBadges(() => route.pathname)
  const manage = useManageSidebar()

  // Same apps doctrine as the rail: keep the query, default off it. `{ data:
  // apps = [] }` discarded the query on the line that made it, so a failed
  // /api/apps silently removed every app's nav entry — the surface just is
  // not there, and nothing anywhere says why.
  const appsQuery = useEnabledApps()
  const appsList = listQuery(appsQuery, { title: 'App links unavailable', variant: 'inline' })
  const appsBroken = $derived(appsList.failed || appsList.stale)

  const { row, manage: manageSection } = $derived(dockSections(user, appsList.rows, denied.current))
  const activePath = $derived(
    // Active state includes the manage section's items: standing on
    // /observability must light the gear, since that is the dock's only
    // marker for the whole manage half of the menu. The most-specific-wins
    // rule over every section at once is the same one the rail used.
    navActiveItem(route.pathname, [...row, ...(manageSection ? [manageSection] : [])])?.to ?? null,
  )
</script>

<!-- h-14 (56px): tile h-9 plus breathing room — the same band weight as the
     rail's 64px column, read horizontally. The dock is shell chrome, not a
     stage header, so it deliberately does not join the h-12 stage-header
     line (UI-CONVENTIONS): the strip below owns that line.
     `shrink-0` keeps it out of the shell's min-h-0 flex chain; the row of
     tiles scrolls in x only, never wrapping and never pushing the right
     cluster off the band. -->
<nav
  aria-label="Primary"
  class="relative z-40 flex h-14 shrink-0 items-center gap-2 border-b border-line bg-sidebar px-2"
>
  <div class="grid h-9 w-9 shrink-0 place-items-center" aria-label="Talaria">
    <WingMark class="h-5 w-5" />
  </div>
  <!-- Only renders inside the Talaria desktop shell — a browser gets
       nothing (feature-detected in the component). -->
  <DesktopSwitcher collapsed />

  <!-- `overflow-x-auto` rather than wrap: a dock wraps into a second row is
       no longer a dock. Room at both ends (`px-1`) for the active tile's
       outset accent band, the rail's lesson carried over. -->
  <div class="flex min-w-0 flex-1 items-center gap-2 overflow-x-auto px-1 py-1">
    {#each row as section, si (section.id)}
      {#if si > 0}<div class="my-2 h-6 w-px shrink-0 bg-line"></div>{/if}
      {#each section.items as item (item.to)}
        {@const badge = badges.badgeFor(item)}
        {@const active = item.to === activePath}
        <RailTooltip label={item.label}>
          <a
            href={item.to}
            data-status={active ? 'active' : undefined}
            aria-label={item.label}
            class={cn(
              'relative grid h-9 w-9 shrink-0 place-items-center rounded-md text-muted transition-colors duration-[120ms] dither-bloom hover:text-fg',
              'data-[status=active]:bg-raised data-[status=active]:text-fg',
              '[&[data-status=active]_svg]:h-[22px] [&[data-status=active]_svg]:w-[22px]',
            )}
          >
            <NavIcon icon={item.icon} />
            {#if badge !== undefined && (badge === null || badge > 0)}
              <!-- `!` (danger) when the count could not be read, never a
                   silent 0: the unread-null doctrine, verbatim from the
                   rail — the dock is the same surface it was. -->
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

    <!-- The gear, after a divider — the position the manage section held at
         the bottom of the rail, restated horizontally. It opens the manage
         sidebar; it does not navigate. Hidden when the sidebar has nothing
         to show (member with zero manage grants), so the affordance never
         opens an empty pane. Lit while the sidebar is open OR while the
         route is inside any manage view, because that is the dock's only
         marker for the whole manage half of the menu. -->
    {#if manageSection}
      <div class="my-2 h-6 w-px shrink-0 bg-line"></div>
      <RailTooltip label="Manage">
        <button
          type="button"
          data-manage-tile
          onclick={toggleManageSidebar}
          aria-label="Manage"
          aria-expanded={manage.manageOpen}
          class={cn(
            'relative grid h-9 w-9 shrink-0 place-items-center rounded-md text-muted transition-colors duration-[120ms] dither-fill hover:text-fg',
            (manage.manageOpen || manageSection.items.some((i) => i.to === activePath)) && 'bg-raised text-fg',
          )}
        >
          <Cog size={16} strokeWidth={1.5} />
        </button>
      </RailTooltip>
    {/if}

    <!-- The apps read decides which entries exist to the left. Dropping its
         failure here is how a surface disappears from the dock with nothing
         to say so — render the notice the query handed us. -->
    {#if appsList.notice}
      <div class="shrink-0"><QueryError {...appsList.notice} /></div>
    {/if}
  </div>

  <!-- The right cluster: everything personal or instance-wide that used to
       live in the rail's footer. The assistant launcher keeps its collapsed
       form (the drawer itself is a peer of the dock in AppLayout, spanning
       below it, exactly the role the rail played); the apps-broken retry
       keeps its tile here, matching the rail's footer placement. -->
  <div class="flex shrink-0 items-center gap-2">
    <SidebarAssistant collapsed />
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
  </div>
</nav>