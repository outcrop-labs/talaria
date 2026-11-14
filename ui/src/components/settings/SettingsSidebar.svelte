<script lang="ts">
  import { Bell, Blocks, KeyRound, LayoutDashboard, Link2, Search, Shield, Sparkles, User, type LucideIcon } from '@lucide/svelte'
  import NavIcon from '@/components/app/NavIcon.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useEnabledApps } from '@/lib/apps'
  import { cn } from '@/lib/cn'
  import { MANAGE_HOME } from '@/lib/manage-area'
  import { markCrossfade } from '@/lib/motion'
  import { NAV, type NavItem } from '@/lib/nav'
  import { badgeTitle, useNavBadges } from '@/lib/nav-badges.svelte'
  import { useAppGrants, useDeniedViews, type SessionUser } from '@/lib/session'
  import {
    ADMIN_HOME,
    OWN_PAGE_TABS,
    searchSettings,
    SETTINGS_HOME,
    settingsNav,
    type SettingsNavGroup,
    type SettingsNavMatch,
    type SettingsNavRow,
    type SettingsPageTabs,
  } from '@/lib/settings-nav'
  import { revealCurrent } from '@/lib/tab-strip'
  import { holdSansScreen, setUiVoice } from '@/lib/ui-voice'
  import { ADMIN_TABS } from '@/routes/app/admin'
  import AppsMark from '@/routes/app/AppsMark.svelte'
  import { MODEL_TABS } from '@/routes/app/models'
  import { OBS_TABS } from '@/routes/app/observability/observability'
  import { navigate, route } from '@/router'

  // The settings area's one navigation: docked beside every page behind the
  // dock's gear and the user menu's Settings and Admin, and absent everywhere
  // else. `user` is the viewer whose grants decide the rows, handed down from
  // the layout so the dock and the sidebar cannot drift.
  let { user }: { user: SessionUser } = $props()

  setUiVoice('sans')

  const denied = useDeniedViews()
  const appGrants = useAppGrants()
  const badges = useNavBadges(() => route.pathname)

  // The apps read decides which app rows exist, so its failure is shown here
  // instead of the rows silently going missing.
  const appsList = listQuery(useEnabledApps(), { title: 'App links unavailable', variant: 'inline' })

  const nav = $derived(
    settingsNav(
      NAV,
      appsList.rows,
      { isAdmin: user.role === 'admin', denied: denied.current, appGrants: appGrants.current },
      route.pathname,
    ),
  )

  $effect(() => {
    if (!nav.inside) return
    return holdSansScreen()
  })

  // The rows this area names itself. NAV and the app manifests carry the rest.
  const ICONS: Record<string, LucideIcon> = {
    [SETTINGS_HOME]: User,
    '/settings/notifications': Bell,
    '/settings/assistant': Sparkles,
    '/settings/connections': Link2,
    '/settings/keys': KeyRound,
    [MANAGE_HOME]: LayoutDashboard,
    [ADMIN_HOME]: Shield,
  }
  // An app's row wears the app's own glyph, so the rows of the App settings group
  // tell apart. It is drawn in the 16px box the drawn icons fill, in the
  // row's own ink.
  const iconOf = (to: string, icon: NavItem['icon'] | undefined): NavItem['icon'] => icon ?? ICONS[to] ?? Blocks

  const PAGE_TABS: SettingsPageTabs = {
    ...OWN_PAGE_TABS,
    '/models': MODEL_TABS,
    '/observability': OBS_TABS,
    [ADMIN_HOME]: ADMIN_TABS,
  }

  let query = $state('')
  const searching = $derived(query.trim().length > 0)
  const matches = $derived(searchSettings(nav.groups, PAGE_TABS, query))
  // Results keep the sidebar's groups, so a match says where it lives.
  const matchGroups = $derived(
    nav.groups.flatMap((g): { group: SettingsNavGroup; matches: SettingsNavMatch[] }[] => {
      const found = matches.filter((m) => m.group === g.id)
      return found.length > 0 ? [{ group: g, matches: found }] : []
    }),
  )

  function onSearchKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      query = ''
    } else if (e.key === 'Enter' && matches[0]) {
      e.preventDefault()
      void navigate(matches[0].to as never)
      query = ''
    }
  }

  const [sendMark, receiveMark] = markCrossfade()

  const groupLabel = 'flex h-7 items-center px-2 font-sans text-xs leading-4 text-muted'
  const rowClass =
    'relative flex h-8 items-center gap-2.5 rounded-md px-2 font-sans text-[13.5px] leading-5 text-muted transition-colors duration-[120ms] hover:bg-[var(--settings-selected)]/50 hover:text-fg focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent'
</script>

{#snippet mark(icon: NavItem['icon'])}
  <span class="relative grid h-4 w-4 shrink-0 place-items-center">
    {#if typeof icon === 'string'}<AppsMark {icon} class="text-sm text-current" />{:else}<NavIcon {icon} />{/if}
  </span>
{/snippet}

{#snippet navRow(row: SettingsNavRow)}
  {@const active = row.to === nav.activePath}
  {@const badge = row.badge ? badges.badgeFor({ ...row, icon: iconOf(row.to, row.icon) }) : undefined}
  <a href={row.to} aria-current={active ? 'page' : undefined} class={cn(rowClass, active && 'font-medium text-fg hover:bg-transparent')}>
    {#if active}
      <span
        aria-hidden="true"
        in:receiveMark={{ key: 'settings-mark' }}
        out:sendMark={{ key: 'settings-mark' }}
        class="absolute inset-0 rounded-md bg-[var(--settings-selected)]"
      >
        <span class="absolute inset-y-2 left-0 w-0.5 rounded-full bg-accent"></span>
      </span>
    {/if}
    {@render mark(iconOf(row.to, row.icon))}
    <span class="relative min-w-0 flex-1 truncate">{row.label}</span>
    {#if badge !== undefined && (badge === null || badge > 0)}
      <span
        title={badgeTitle(badge)}
        class={cn('relative font-sans text-xs tabular-nums', badge === null ? 'text-danger' : 'text-muted')}
      >
        {badge === null ? '!' : badge}
      </span>
    {/if}
  </a>
{/snippet}

{#if nav.inside}
  <aside data-voice="sans" class="hidden w-60 shrink-0 flex-col border-r border-line bg-sidebar md:flex" aria-label="Settings">
    <div class="shrink-0 p-3 pb-0">
      <label class="relative block">
        <Search size={14} class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted" />
        <input
          type="search"
          bind:value={query}
          onkeydown={onSearchKey}
          placeholder="Search settings"
          aria-label="Search settings"
          class="h-8 w-full rounded-md border border-line bg-[var(--theme-input)] pl-8 pr-2.5 font-sans text-[13px] text-fg outline-none transition-colors placeholder:text-muted focus:border-accent focus:ring-2 focus:ring-accent-soft [&::-webkit-search-cancel-button]:hidden"
        />
      </label>
    </div>

    <nav class="min-h-0 flex-1 overflow-y-auto p-3" aria-label="Settings pages">
      {#if searching}
        {#if matches.length === 0}
          <p class="px-2 py-1.5 font-sans text-[13px] leading-5 text-muted">No settings match</p>
        {:else}
          <div class="space-y-5">
            {#each matchGroups as { group, matches: found } (group.id)}
              <section aria-label={group.label}>
                <h2 class={groupLabel}>{group.label}</h2>
                <ul class="space-y-0.5">
                  {#each found as m (m.to + (m.parent ?? ''))}
                    <li>
                      <a href={m.to} onclick={() => (query = '')} class={rowClass}>
                        {@render mark(iconOf(m.rowTo, m.icon))}
                        <span class="min-w-0 flex-1 truncate">
                          {#if m.parent}{m.parent} <span aria-hidden="true">›</span> <span class="text-fg">{m.label}</span>{:else}{m.label}{/if}
                        </span>
                      </a>
                    </li>
                  {/each}
                </ul>
              </section>
            {/each}
          </div>
        {/if}
      {:else}
        <div class="space-y-5">
          {#each nav.groups as group (group.id)}
            <section aria-label={group.label}>
              <h2 class={groupLabel}>{group.label}</h2>
              <ul class="space-y-0.5">
                {#each group.rows as row (row.to)}
                  <li>{@render navRow(row)}</li>
                {/each}
              </ul>
            </section>
          {/each}
        </div>
      {/if}

      {#if appsList.notice}
        <div class="mt-4 px-2"><QueryError {...appsList.notice} /></div>
      {/if}
    </nav>
  </aside>

  <!-- Below md a 240px column would take most of a phone, so the same rows
       become a strip across the top that scrolls sideways. -->
  <nav
    data-voice="sans"
    use:revealCurrent
    aria-label="Settings"
    class="flex shrink-0 items-center gap-1 overflow-x-auto border-b border-line bg-sidebar px-2 py-1.5 md:hidden"
  >
    {#each nav.groups as group (group.id)}
      {#each group.rows as row (row.to)}
        {@const active = row.to === nav.activePath}
        <a
          href={row.to}
          aria-current={active ? 'page' : undefined}
          class={cn(
            'relative flex h-8 shrink-0 items-center gap-1.5 rounded-md px-2.5 font-sans text-[13.5px] text-muted transition-colors duration-[120ms] hover:text-fg',
            active && 'bg-[var(--settings-selected)] font-medium text-fg',
          )}
        >
          {#if active}<span aria-hidden="true" class="absolute inset-y-2 left-0 w-0.5 rounded-full bg-accent"></span>{/if}
          {@render mark(iconOf(row.to, row.icon))}
          <span>{row.label}</span>
        </a>
      {/each}
    {/each}
  </nav>
{/if}
