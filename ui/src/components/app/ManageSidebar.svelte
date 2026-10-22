<script lang="ts">
  import { LayoutDashboard } from '@lucide/svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import NavIcon from './NavIcon.svelte'
  import { markCrossfade } from '@/lib/motion'
  import { ditherSurface } from '@/lib/dither-surface'
  import { badgeTitle, useNavBadges } from '@/lib/nav-badges.svelte'
  import { manageArea } from '@/lib/manage-area'
  import { listQuery } from '@/components/ui/query-state'
  import { useEnabledApps } from '@/lib/apps'
  import { useDeniedViews, type SessionUser } from '@/lib/session'
  import { NAV, type NavItem } from '@/lib/nav'
  import { cn } from '@/lib/cn'
  import { route } from '@/router'

  // THE MANAGE SIDEBAR — the control plane's own navigation, DOCKED on every
  // manage page and absent everywhere else. It used to be a pane the dock's
  // gear slid out over whatever view you were on, with a pin to keep it
  // open; that made the control plane a gesture you had to hold. Now the gear
  // goes to /manage, the overview, and the manage area simply has a sidebar
  // the way Boards and Work have theirs: always there, never in the way of a
  // work view, nothing to pin.
  //
  // Same row grammar as the rail it came from (208px, the section header,
  // 30px rows with the nav-bar accent), so a person who knew the rail still
  // recognizes it. Below `md` a 208px column would take most of a phone, so
  // the same rows become a strip across the top of the page.
  //
  // `user` is the viewer whose grants decide the rows — the value the dock
  // derives from, handed down so the two cannot drift.
  let { user }: { user: SessionUser } = $props()

  const denied = useDeniedViews()
  const badges = useNavBadges(() => route.pathname)

  // The apps read, same doctrine as the rail: keep the query, default off it,
  // surface the failure. App manage surfaces slot into the manage section.
  const appsQuery = useEnabledApps()
  const appsList = listQuery(appsQuery, { title: 'App links unavailable', variant: 'inline' })

  const area = $derived(
    manageArea(NAV, appsList.rows, { isAdmin: user.role === 'admin', denied: denied.current }, route.pathname),
  )

  const OVERVIEW: NavItem = { to: '/manage', label: 'Overview', icon: LayoutDashboard }
  const rows = $derived(area.items.length > 0 ? [OVERVIEW, ...area.items] : [])

  // ONE MARK FOR THE WHOLE LIST: the selected tile travels between rows
  // rather than vanishing here and appearing there.
  const [sendMark, receiveMark] = markCrossfade()

  const statusFor = (item: NavItem): 'active' | undefined => (item.to === area.activePath ? 'active' : undefined)
</script>

{#if area.inside && rows.length > 0}
  <!-- md and up: a docked column beside the page. In the layout's flow, not
       an overlay — the page is narrower by exactly the sidebar, and nothing
       covers it. -->
  <aside class="hidden w-[208px] shrink-0 flex-col border-r border-line bg-sidebar md:flex" aria-label="Manage">
    <div class="flex h-9 shrink-0 items-center pl-2">
      <!-- The section header, the rail's own voice: 10px mono uppercase. -->
      <div class="flex h-6 items-center px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Manage
      </div>
    </div>

    <div class="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto p-1">
      <!-- `space-y-1.5` rather than `space-y-px`: the row treatment blooms
           OUTWARD — the rail's lesson, carried verbatim. -->
      <ul class="space-y-1.5">
        {#each rows as item (item.to)}
          {@const badge = badges.badgeFor(item)}
          <li>
            <a
              {@attach ditherSurface()}
              href={item.to}
              data-status={statusFor(item)}
              aria-current={statusFor(item) ? 'page' : undefined}
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
                <!-- Nav counts are muted, not accent. The unreadable case is
                     the one exception: it is not a count, and a muted "!"
                     reads as decoration. -->
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

      <!-- The apps read decides which app entries exist above. Dropping its
           failure here is how a surface disappears with nothing to say so. -->
      {#if appsList.notice}
        <div class="shrink-0 px-1"><QueryError {...appsList.notice} /></div>
      {/if}
    </div>
  </aside>

  <!-- Below md: the same rows as a strip across the top, scrolling sideways
       rather than wrapping, so the page below keeps its height. -->
  <nav
    aria-label="Manage"
    class="flex shrink-0 items-center gap-1 overflow-x-auto border-b border-line bg-sidebar px-2 py-1.5 md:hidden"
  >
    {#each rows as item (item.to)}
      <a
        href={item.to}
        data-status={statusFor(item)}
        aria-current={statusFor(item) ? 'page' : undefined}
        class={cn(
          'flex h-8 shrink-0 items-center gap-1.5 rounded-md px-2.5 font-sans text-[13px] text-muted transition-colors duration-[120ms] hover:text-fg',
          'data-[status=active]:bg-raised data-[status=active]:font-medium data-[status=active]:text-fg',
        )}
      >
        <span class="grid h-4 w-4 shrink-0 place-items-center"><NavIcon icon={item.icon} /></span>
        <span>{item.label}</span>
      </a>
    {/each}
  </nav>
{/if}
