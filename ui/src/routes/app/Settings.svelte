<script lang="ts">
  import { tabFromPath } from '@/lib/route-tabs'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigate, route } from '@/router'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import Panel from '@/components/ui/Panel.svelte'
  import { useDeniedViews, useSession } from '@/lib/session'
  import AssistantSection from '@/components/assistant/AssistantSection.svelte'
  import AppSurface from '@/components/app/AppSurface.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import Tabs from '@/components/ui/Tabs.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import PreferredModelPicker from './settings/PreferredModelPicker.svelte'
  import NotificationsSection from './settings/NotificationsSection.svelte'
  import McpConnectionsSection from './settings/McpConnectionsSection.svelte'
  import IntegrationsSection from './settings/IntegrationsSection.svelte'
  import ApiKeysSection from './settings/ApiKeysSection.svelte'

  // Personal settings, tabbed by concern: Profile (identity + drafting model),
  // Assistant (the member's whole personal agent), Connections, API keys —
  // plus one tab per enabled app that ships a settings surface.

  type SettingsTab = 'profile' | 'notifications' | 'assistant' | 'connections' | 'keys' | `app:${string}`
  const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
    { id: 'profile', label: 'Profile' },
    { id: 'notifications', label: 'Notifications' },
    { id: 'assistant', label: 'Assistant' },
    { id: 'connections', label: 'Connections' },
    { id: 'keys', label: 'API keys' },
  ]

  const qc = useQueryClient()
  const sessionQuery = useSession()
  const user = $derived(sessionQuery.data)
  const sessionLoading = $derived(sessionQuery.isLoading)
  let name = $state('')
  const savedFlash = useSavedFlash()
  let busy = $state(false)
  // /settings/assistant deep-links a tab. `app:<slug>` tabs come from
  // enabled apps' settings surfaces — validated at render (the app list is
  // async), unknown slugs fall back to Profile.
  // THE URL IS THE TAB — /settings and /settings/<tab>. App-provided tabs are
  // keyed `app:<slug>`, a legal path segment, so they route like any other:
  // `tabFromPath` covers the static set and the pattern covers the dynamic
  // ones, which cannot be enumerated here (they depend on which apps this
  // person is granted).
  const tab = $derived.by((): SettingsTab => {
    const seg = tabFromPath(route.pathname, '/settings', SETTINGS_TABS.map((v) => v.id), 'profile')
    if (seg !== 'profile') return seg
    const raw = route.pathname.startsWith('/settings/') ? decodeURIComponent(route.pathname.slice('/settings/'.length)) : ''
    return /^app:[a-z0-9-]+$/.test(raw) ? (raw as SettingsTab) : 'profile'
  })
  const setTab = (t: SettingsTab) => {
    if (t === 'profile') void navigate('/settings')
    else void navigate('/settings/:tab', { params: { tab: t } })
  }
  // Enabled apps with a settings surface get their own tab, labeled by the
  // app — only for people granted the app (apps are explicit-grant).
  // A failed /api/apps used to remove app settings TABS with no trace — the
  // settings for an app you have simply were not there.
  const appsList = listQuery(useEnabledApps(), { title: 'Could not load app settings tabs', variant: 'inline' })
  const deniedForApps = useDeniedViews()
  const appTabs = $derived(
    appsList.rows
      .filter((a) => a.surfaces.settings && !deniedForApps.current.includes(`/x/${a.slug}`))
      .map((a) => ({ id: `app:${a.slug}` as SettingsTab, label: a.surfaces.settings! })),
  )

  $effect(() => {
    if (user) name = user.name ?? ''
  })

  const save = async () => {
    const n = name.trim()
    if (!n || n === user?.name) return
    busy = true
    try {
      const r = await fetch('/api/profile', {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name: n }),
      })
      if (r.ok) {
        await qc.invalidateQueries({ queryKey: ['session'] })
        await qc.invalidateQueries({ queryKey: ['users'] })
        savedFlash.flash()
      }
    } finally {
      busy = false
    }
  }
</script>

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto w-full max-w-2xl">
    <h1 class="mb-4 text-2xl font-semibold tracking-tight text-fg">Settings</h1>
    <Tabs items={[...SETTINGS_TABS, ...appTabs]} value={tab} onChange={setTab} class="mb-6" />
    {#if appsList.notice}<div class="-mt-4 mb-6"><QueryError {...appsList.notice} /></div>{/if}
    <!-- Tab-pane grammar: pane rises in on switch (no exit) and its sections
         stagger — settings panes are section stacks. Keying is safe here: the
         profile input's state (`name`) lives in this component, not the pane.
         No AutoHeight — the pane is the last thing on a scrolling page, so its
         resize has nothing below it to displace. -->
    {#key tab}
    <div in:fly={{ y: 6, duration: 200 }} use:staggerIn class="space-y-6">
    {#if tab === 'profile'}
      <Panel as="section">
        <div class="mb-4 flex items-center gap-3">
          {#if sessionLoading}
            <!-- Hold the identity header until the session lands — an empty
                 avatar/name/email that fills in later reads as a glitch. -->
            <Skeleton class="h-10 w-10 shrink-0 rounded-full" />
            <div class="min-w-0 flex-1 space-y-2">
              <Skeleton class="h-3 w-36 rounded-full" delay={0.12} />
              <Skeleton class="h-2.5 w-48 rounded-full" delay={0.24} />
            </div>
          {:else}
            <Avatar src={user?.picture} name={name || user?.email} class="h-10 w-10" />
            <div class="min-w-0">
              <div class="truncate text-sm font-medium text-fg">{name || user?.email}</div>
              <div class="truncate text-xs text-muted">{user?.email}</div>
            </div>
          {/if}
        </div>
        <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Display name</label>
        {#if sessionLoading}
          <div class="flex items-center gap-2">
            <Skeleton class="h-11 flex-1" />
            <Skeleton class="h-11 w-20" delay={0.12} />
          </div>
        {:else}
          <div class="flex items-center gap-2">
            <Input
              bind:value={name}
              onkeydown={(e) => e.key === 'Enter' && void save()}
              placeholder="How teammates and agents see you"
            />
            <Button onclick={() => void save()} disabled={busy || !name.trim() || name.trim() === user?.name}>
              Save
            </Button>
          </div>
        {/if}
        {#if savedFlash.saved}<div class="mt-2 text-xs text-success">Saved</div>{/if}
        <PreferredModelPicker />
      </Panel>
    {/if}

    {#if tab === 'notifications'}<NotificationsSection />{/if}
    {#if tab === 'assistant'}<AssistantSection />{/if}
    {#if tab === 'connections'}
      <IntegrationsSection />
      <McpConnectionsSection />
    {/if}
    {#if tab === 'keys'}<ApiKeysSection />{/if}
    {#if tab.startsWith('app:')}<AppSurface slug={tab.slice(4)} surface="settings" />{/if}
    </div>
    {/key}
  </div>
</div>
