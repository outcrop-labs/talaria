<script lang="ts" module>
  import type { SessionUser } from '@/lib/session'

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
  const isInbox = $derived(pathname === '/' || pathname === '/inbox')
  const inboxQueue = useInboxFocus(() => ({ enabled: isInbox }))
  const inboxSummary = useInboxFocusSummary(() => ({ enabled: !isInbox }))
  // `null` = the count could NOT be read, which is a different fact from zero
  // and must not render as one. The rail is where a person checks whether
  // anything is waiting without opening anything; a silent 0 over a failed
  // read is this app's oldest bug shape, on the one surface that gates every
  // human decision. `!` with a title is the honest badge — see the Inbox
  // surface itself (components/inbox/FocusInbox.svelte) for the same rule.
  const inboxRead = $derived(isInbox ? inboxQueue : inboxSummary)
  const unread: number | null = $derived(
    inboxRead.isError && inboxRead.data === undefined
      ? null
      : isInbox
        ? (inboxQueue.data?.counts.total ?? 0)
        : (inboxSummary.data?.count ?? 0),
  )
  const unreadLabel = $derived(unread === null ? '!' : String(unread))
  const unreadTitle = $derived(unread === null ? 'Could not load what is waiting for you — open the Inbox' : undefined)
  const showUnread = $derived(unread === null || unread > 0)
  // Enabled apps slot into the sections as if they shipped with the platform:
  // work surfaces under Work, manage surfaces under Manage (grant-gated like
  // any core Manage view via deniedViews).
  // Query handling only. `{ data: apps = [] }` discarded the query on the line
  // that made it, so a failed /api/apps silently removed every app's nav entry
  // — the surface just is not in the rail, and nothing anywhere says why.
  const appsQuery = useEnabledApps()
  const appsList = listQuery(appsQuery, { title: 'App links unavailable', variant: 'inline' })
  const appsBroken = $derived(appsList.failed || appsList.stale)
  const appItems: Record<string, NavItem[]> = $derived({
    Work: appsList.rows.filter((a) => a.surfaces.work).map((a) => ({ to: `/x/${a.slug}`, label: a.surfaces.work!, icon: a.icon })),
    Manage: appsList.rows.filter((a) => a.surfaces.manage).map((a) => ({ to: `/x/${a.slug}/manage`, label: a.surfaces.manage!, icon: a.icon })),
  })

  // Denied-view + role filtering, shared by both modes.
  const sections = $derived.by(() =>
    NAV.flatMap((section) => {
      if (section.adminOnly && !isAdmin) return []
      const items = [...section.items, ...(appItems[section.title] ?? [])].filter(
        (i) => (!i.adminOnly || isAdmin) && !denied.current.includes(i.to) && !denied.current.some((d) => i.to.startsWith(d + '/')),
      )
      return items.length === 0 ? [] : [{ title: section.title, items }]
    }),
  )

  // Exact for Home and for app WORK items: /x/<slug> is a path prefix of its
  // sibling /x/<slug>/manage, and fuzzy matching would light both up at once.
  const exactFor = (item: NavItem) => item.to === '/' || appItems.Manage!.some((m) => m.to.startsWith(item.to + '/'))

  // The active match feeding the data-status attribute (was TanStack Link's
  // activeOptions job — same exact/fuzzy semantics).
  const statusFor = (item: NavItem): 'active' | undefined =>
    (exactFor(item) ? pathname === item.to : pathname === item.to || pathname.startsWith(item.to + '/'))
      ? 'active'
      : undefined
</script>

{#snippet modals()}
  <CreateBoardModal open={creating} onClose={() => (creating = false)} />
  <TeamsModal open={teamsOpen} onClose={() => (teamsOpen = false)} />
{/snippet}

<!-- CollapsePane owns the collapse/expand width glide (the two variants used
     to be separate <nav>s, so the flip snapped with no animation possible).
     Inner divs pin each variant's width so content clips during the glide
     instead of squishing. -->
<CollapsePane tag="nav" collapsed={nav.collapsed} width="w-[208px]" collapsedWidth="w-16" class="h-full shrink-0 border-r border-line bg-sidebar">
{#if nav.collapsed}
  <!-- ── Icon rail (64px, spec §5) ───────────────────────────────────────── -->
  <div class="flex h-full w-16 flex-col items-center pb-5 pt-3">
    <div class="grid h-9 w-9 shrink-0 place-items-center" aria-label="Talaria">
      <WingMark class="h-5 w-5" />
    </div>

    <div class="mt-3 flex min-h-0 w-full flex-1 flex-col items-center gap-1 overflow-y-auto">
      {#each sections as section, si (section.title)}
        {#if si > 0}<div class="my-2 h-px w-6 shrink-0 bg-line"></div>{/if}
        {#each section.items as item (item.to)}
          <RailTooltip label={item.label}>
            <a
              href={item.to}
              data-view-transition
              data-status={statusFor(item)}
              aria-label={item.label}
              class={cn(
                'relative grid h-9 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg',
                'data-[status=active]:bg-raised data-[status=active]:text-fg',
                '[&[data-status=active]_svg]:h-[22px] [&[data-status=active]_svg]:w-[22px]',
              )}
            >
              <NavIcon icon={item.icon} />
              {#if item.to === '/' && showUnread}
                <span
                  title={unreadTitle}
                  class={cn(
                    'absolute right-0.5 top-0 font-mono text-[10px] leading-3 tracking-[0.05em]',
                    unread === null ? 'text-[color:var(--theme-danger)]' : 'text-muted',
                  )}
                >
                  {unreadLabel}
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
        <RailTooltip label="App links unavailable — retry">
          <button
            type="button"
            onclick={() => void appsQuery.refetch()}
            aria-label="App links unavailable — retry"
            class="grid h-9 w-9 place-items-center rounded-md text-danger transition-colors duration-[120ms] hover:bg-hover"
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
          class="grid h-9 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg"
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

    <div class="mt-3 flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto">
      {#each sections as section (section.title)}
        <div>
          <div class="flex h-6 items-center px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            {section.title}
          </div>
          <ul class="space-y-px">
            {#each section.items as item (item.to)}
              <li>
                <a
                  href={item.to}
                  data-view-transition
                  data-status={statusFor(item)}
                  class={cn(
                    'flex h-[30px] items-center gap-[9px] rounded-md px-2 font-sans text-[13px] leading-4 text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg',
                    'data-[status=active]:bg-raised data-[status=active]:font-medium data-[status=active]:text-fg',
                    '[&[data-status=active]_.nav-bar]:bg-accent [&[data-status=active]_.nav-ico]:text-fg',
                  )}
                >
                  <span class="nav-bar h-3.5 w-[3px] shrink-0 rounded-[2px] bg-transparent" aria-hidden="true"></span>
                  <span class="nav-ico grid h-4 w-4 shrink-0 place-items-center text-muted">
                    <NavIcon icon={item.icon} />
                  </span>
                  <span class="flex-1 truncate">{item.label}</span>
                  {#if item.to === '/' && showUnread}
                    <!-- Spec §5: nav counts are muted (#8E877E) — not accent.
                         The unreadable case is the one exception: it is not a
                         count, and a muted "!" reads as decoration. -->
                    <span
                      title={unreadTitle}
                      class={cn(
                        'font-mono text-[10px] leading-3 tracking-[0.05em]',
                        unread === null ? 'text-[color:var(--theme-danger)]' : 'text-muted',
                      )}
                    >
                      {unreadLabel}
                    </span>
                  {/if}
                </a>
                {#if item.to === '/boards' && pathname.startsWith('/boards')}
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
        class="ml-auto grid h-5 w-5 shrink-0 place-items-center rounded text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg"
      >
        <ChevronsLeft size={13} strokeWidth={1.5} />
      </button>
    </div>

    {@render modals()}
  </div>
{/if}
</CollapsePane>
