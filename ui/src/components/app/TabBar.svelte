<script lang="ts" module>
  import type { SessionUser } from '@/lib/session'
  import { navSections, navActiveItem, appWorkItem, appManageItem } from '@/lib/nav-sections'
  import { NAV } from '@/lib/nav'

  // The section derivation the sidebar shares, wired for the bar's narrower
  // question: which items are tabs at all? Manage is not — the manage views
  // live behind the gear in the right cluster, which goes to the /manage
  // overview, and in the sidebar docked on every manage page. So the strip
  // is Work, then Apps.
  function dockSections(
    user: SessionUser,
    appsRows: { slug: string; icon: string; surfaces: { work?: string; manage?: string } }[],
    grants: { denied: readonly string[]; appGrants: readonly string[] | null },
  ) {
    const appWork = appsRows.flatMap((a) => {
      const item = appWorkItem(a)
      return item === null ? [] : [item]
    })
    const appManage = appsRows.flatMap((a) => {
      const item = appManageItem(a)
      return item === null ? [] : [item]
    })
    // navSections appends app manage entries to the section whose id is
    // 'manage', which we keep in the result so the sidebar and the bar see
    // one derivation, not two.
    const sections = navSections(NAV, appWork, appManage, { isAdmin: user.role === 'admin', ...grants })
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
  import type { NavItem } from '@/lib/nav'
  import NavIcon from './NavIcon.svelte'
  import RailTooltip from './RailTooltip.svelte'
  import DesktopSwitcher from './DesktopSwitcher.svelte'
  import SidebarAssistant from './SidebarAssistant.svelte'
  import BoardsSearch from './BoardsSearch.svelte'
  import NotificationBell from './NotificationBell.svelte'
  import UserMenu from './UserMenu.svelte'
  import { isUnder } from '@/lib/route-tabs'
  import { badgeTitle, useNavBadges } from '@/lib/nav-badges.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import { useAppGrants, useDeniedViews } from '@/lib/session'
  import { manageArea, MANAGE_HOME } from '@/lib/manage-area'
  import { edgeFade } from '@/lib/tab-strip'
  import { cn } from '@/lib/cn'
  import { p, route } from '@/router'

  // THE TAB BAR, the shell's primary nav and its only chrome row. The open
  // view is a tab cut from the page surface: the page's colour, rounded on
  // top, curving out at its feet, breaking the bar's bottom hairline so the
  // bar reads as the top of the window and the tab as the sheet in front.
  // Every other view is a small icon tile. Manage gets no tab: it lives
  // behind one gear in the right cluster, beside the other instance-level
  // controls. Settings and Admin get none either: the account chip lights
  // for them.
  let { user, onLogout }: { user: SessionUser; onLogout: () => void } = $props()

  const denied = useDeniedViews()
  const appGrants = useAppGrants()
  const badges = useNavBadges(() => route.pathname)

  // Keep the query, default off it: a failed /api/apps must say so here, not
  // silently remove every app's tab.
  const appsQuery = useEnabledApps()
  const appsList = listQuery(appsQuery, { title: 'App links unavailable', variant: 'inline' })
  const appsBroken = $derived(appsList.failed || appsList.stale)

  // The boards view carries its own search in its toolbar.
  const onBoardsHere = $derived(isUnder(route.pathname, '/boards'))
  // Settings and Admin render no title of their own, so the chip that owns
  // them lights to say where you are, as the gear does for the manage half.
  const onSystemHere = $derived(isUnder(route.pathname, '/settings') || isUnder(route.pathname, '/admin'))

  const grants = $derived({ denied: denied.current, appGrants: appGrants.current })
  const { row, manage: manageSection } = $derived(dockSections(user, appsList.rows, grants))
  const manage = $derived(
    manageArea(NAV, appsList.rows, { isAdmin: user.role === 'admin', ...grants }, route.pathname),
  )
  const activePath = $derived(
    // The manage items join the contest though they have no tab: an app's
    // manage surface (/x/<app>/manage) must beat its own work tab (/x/<app>)
    // by the most-specific rule, or standing on it would open the work tab.
    navActiveItem(route.pathname, [...row, ...(manageSection ? [manageSection] : [])])?.to ?? null,
  )
</script>

{#snippet face(item: NavItem, size: number)}
  <NavIcon icon={item.icon} {size} />
{/snippet}

{#snippet badgeMark(badge: number | null, className: string)}
  <!-- `!` when the count could not be read, never a silent 0. -->
  <span
    title={badgeTitle(badge)}
    class={cn(
      'font-mono text-[10px] leading-3 tracking-[0.05em]',
      badge === null ? 'text-[color:var(--theme-danger)]' : 'text-muted',
      className,
    )}
  >
    {badge === null ? '!' : badge}
  </span>
{/snippet}

<!-- h-14 is shell chrome, not a stage header, so it does not join the h-12
     stage-header line (UI-CONVENTIONS). `@container` drives the wordmark from
     the bar's own width, which the desktop shell can narrow. -->
<nav aria-label="Primary" class="@container relative z-40 flex h-14 shrink-0 items-stretch gap-2 bg-sidebar px-2">
  <!-- The hairline the open tab breaks through, as its own layer under the
       strip: a border on the nav would paint over the tab. -->
  <div class="pointer-events-none absolute inset-x-0 bottom-0 h-px bg-line" aria-hidden="true"></div>

  <div class="flex shrink-0 items-center gap-2">
    <div class="flex h-9 shrink-0 items-center gap-2.5 pl-1" aria-label="Talaria">
      <WingMark size={28} />
      <span
        class="hidden font-sans text-[13px] font-semibold leading-4 tracking-[0.18em] text-fg @4xl:block"
        aria-hidden="true">TALARIA</span
      >
    </div>
    <DesktopSwitcher collapsed />
  </div>

  <!-- Scrolls in x rather than wrapping, so the right cluster never leaves
       the band. `px-3` is room for the open tab's feet at either end. -->
  <div class="tab-strip relative flex min-w-0 flex-1 items-end gap-2 overflow-x-auto px-3" use:edgeFade>
    {#each row as section, si (section.id)}
      {#if si > 0}<div class="mb-3 h-5 w-px shrink-0 bg-line"></div>{/if}
      {#each section.items as item (item.to)}
        {@const badge = badges.badgeFor(item)}
        {#if item.to === activePath}
          <a
            href={item.to}
            aria-current="page"
            class="ears-bottom flex h-11 min-w-0 max-w-[15rem] shrink-0 items-center gap-2 rounded-t-[10px] border border-b-0 border-line bg-surface pl-3 pr-4 text-fg focus-visible:outline-offset-[-2px]"
          >
            {@render face(item, 18)}
            <span class="truncate font-sans text-[14px] font-medium leading-[1.25]">{item.label}</span>
            {#if badge !== undefined && (badge === null || badge > 0)}{@render badgeMark(badge, 'shrink-0')}{/if}
          </a>
        {:else}
          <RailTooltip label={item.label}>
            <a
              href={item.to}
              aria-label={item.label}
              class="relative mb-1.5 grid h-8 w-8 shrink-0 place-items-center rounded-md text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
            >
              {@render face(item, 16)}
              {#if badge !== undefined && (badge === null || badge > 0)}{@render badgeMark(badge, 'absolute right-0 top-0')}{/if}
            </a>
          </RailTooltip>
        {/if}
      {/each}
    {/each}

    {#if appsList.notice}
      <div class="mb-1.5 shrink-0 self-end"><QueryError {...appsList.notice} /></div>
    {/if}
  </div>

  <!-- THE RIGHT CLUSTER, read right to left: who you are, what is waiting
       for you, what you are looking for, then the workspace affordances. The
       divider is the seam between the workspace and the person. -->
  <div class="flex shrink-0 items-center gap-2">
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
    <SidebarAssistant collapsed />
    <!-- A link to the /manage overview, lit anywhere in the manage area, and
         absent for a member with no manage grants so it never opens onto an
         empty room. -->
    {#if manage.items.length > 0}
      <RailTooltip label="Manage">
        <a
          href={p(MANAGE_HOME)}
          aria-label="Manage"
          data-status={manage.inside ? 'active' : undefined}
          aria-current={manage.inside ? 'page' : undefined}
          class={cn(
            'relative grid h-9 w-9 shrink-0 place-items-center rounded-md text-muted transition-colors duration-[120ms] dither-fill hover:text-fg',
            'data-[status=active]:bg-raised data-[status=active]:text-fg',
          )}
        >
          <Cog size={16} strokeWidth={1.5} />
        </a>
      </RailTooltip>
    {/if}
    <div class="my-2 h-6 w-px shrink-0 bg-line"></div>
    {#if !onBoardsHere}
      <BoardsSearch />
    {/if}
    <NotificationBell />
    <UserMenu {user} {onLogout} active={onSystemHere} hasManage={manage.items.length > 0} />
  </div>
</nav>
