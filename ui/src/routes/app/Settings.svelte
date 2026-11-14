<script lang="ts">
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import { tabFromPath } from '@/lib/route-tabs'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { route } from '@/router'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import { useAppGrants, useSession } from '@/lib/session'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import AssistantSection from '@/components/assistant/AssistantSection.svelte'
  import AppSurface from '@/components/app/AppSurface.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import { appAccess } from '@/lib/app-grants'
  import { fly, staggerIn } from '@/lib/motion'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import PreferredModelPicker from './settings/PreferredModelPicker.svelte'
  import TimezonePicker from './settings/TimezonePicker.svelte'
  import ProfilePhotoField from './settings/ProfilePhotoField.svelte'
  import TitleField from './settings/TitleField.svelte'
  import NotificationsSection from './settings/NotificationsSection.svelte'
  import IntegrationsSection from './settings/IntegrationsSection.svelte'
  import ApiKeysSection, { useApiKeys } from './settings/ApiKeysSection.svelte'
  import DesktopSection from './settings/DesktopSection.svelte'
  import { inDesktopShell } from '@/lib/desktop-shell'

  // Personal settings, one page per concern: Profile (identity + drafting
  // model), Notifications, Assistant (the member's whole personal agent),
  // Connections, API keys — plus one per enabled app that ships a settings
  // surface. The sidebar's Personal group is the way between them.
  type SettingsTab = 'profile' | 'notifications' | 'assistant' | 'connections' | 'keys' | `app:${string}`
  const SETTINGS_TABS: { id: SettingsTab; label: string; blurb: string }[] = [
    { id: 'profile', label: 'Profile', blurb: 'How you appear to teammates and agents.' },
    { id: 'notifications', label: 'Notifications', blurb: 'Choose what reaches you, and where.' },
    { id: 'assistant', label: 'Assistant', blurb: 'Your personal agent, its voice, and what it can reach.' },
    { id: 'connections', label: 'Connections', blurb: 'Accounts your assistant can act through.' },
    { id: 'keys', label: 'API keys', blurb: 'Keys for scripts and tools that call Talaria as you.' },
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
  // Enabled apps with a settings surface get their own tab, labeled by the
  // app — only for people granted it. A settings tab follows the app's
  // MANAGE grant (app-grants.ts); it used to follow the work view, which a
  // settings-only app never has, so its tab showed for every member.
  // A failed /api/apps used to remove app settings TABS with no trace — the
  // settings for an app you have simply were not there.
  const appsList = listQuery(useEnabledApps(), { title: 'Could not load app settings tabs', variant: 'inline' })
  const appGrants = useAppGrants()
  const appTabs = $derived(
    appsList.rows
      .filter((a) => appAccess(a, { isAdmin: user?.role === 'admin', appGrants: appGrants.current }).settings)
      .map((a) => ({ id: `app:${a.slug}` as SettingsTab, label: a.surfaces.settings!, blurb: undefined })),
  )
  // An app tab this person was not granted has no page: the URL alone
  // (/settings/app:<slug>) is not a grant, so it titles as Settings.
  const page = $derived(
    [...SETTINGS_TABS, ...appTabs].find((t) => t.id === tab) ?? { id: tab, label: 'Settings', blurb: undefined },
  )

  $effect(() => {
    if (user) name = user.name ?? ''
  })

  // API keys: the page's one create lives in its header, and only for someone
  // who may create a key. The list's own read says so.
  const apiKeys = useApiKeys(() => tab === 'keys')
  let creatingKey = $state(false)

  const save = async () => {
    const n = name.trim()
    if (!n || n === user?.name) return
    busy = true
    try {
      await putJson<{ ok: true }>(`/api/me`, { name: n })
      await qc.invalidateQueries({ queryKey: ['session'] })
      await qc.invalidateQueries({ queryKey: ['users'] })
      savedFlash.flash()
    } catch (e) {
      toastError('Could not save your profile', e)
    } finally {
      busy = false
    }
  }
</script>

{#snippet keyActions()}
  <Button size="sm" onclick={() => (creatingKey = true)}>Create key</Button>
{/snippet}

<SettingsPage
  title={page.label}
  blurb={page.blurb}
  autosave={tab === 'profile'}
  saved={savedFlash.saved}
  actions={tab === 'keys' && apiKeys.data?.canMint ? keyActions : undefined}
>
    {#if appsList.notice}<QueryError {...appsList.notice} />{/if}
    <!-- Pane grammar: the pane rises in on switch (no exit) and its sections
         stagger. Keying is safe here: the profile input's state (`name`)
         lives in this component, not the pane. -->
    {#key tab}
    <div in:fly={{ y: 6, duration: 200 }} use:staggerIn class="space-y-8">
    {#if tab === 'profile'}
      <SettingsGroup>
        {#if sessionLoading}
          <!-- Hold the rows until the session lands: an empty name that fills
               in later reads as a glitch. -->
          <SettingsRow label="Photo"><Skeleton class="h-9 w-9 rounded-full" /><Skeleton class="h-8 w-20" /></SettingsRow>
          <SettingsRow label="Name"><Skeleton class="h-9 w-[280px]" /></SettingsRow>
        {:else}
          <ProfilePhotoField {user} />
          <SettingsRow label="Name" hint="How teammates and agents see you." as="label">
            <Input
              size="sm"
              class="w-[280px]"
              bind:value={name}
              disabled={busy}
              onblur={() => void save()}
              onkeydown={inlineEditKeys(() => (name = user?.name ?? ''))}
              placeholder="Your name"
            />
          </SettingsRow>
          <SettingsRow label="Email">
            <span class="font-sans text-sm leading-5 text-muted">{user?.email}</span>
          </SettingsRow>
        {/if}
        <TitleField />
      </SettingsGroup>
      <SettingsSection title="Preferences">
        <SettingsGroup>
          <PreferredModelPicker />
          <TimezonePicker />
        </SettingsGroup>
      </SettingsSection>
      {#if inDesktopShell()}<DesktopSection />{/if}
    {/if}

    {#if tab === 'notifications'}<NotificationsSection />{/if}
    {#if tab === 'assistant'}<div class="space-y-6"><AssistantSection /></div>{/if}
    {#if tab === 'connections'}<IntegrationsSection />{/if}
    {#if tab === 'keys'}<ApiKeysSection bind:creating={creatingKey} />{/if}
    <!-- Only a tab this person was granted mounts its surface: the URL alone
         (/settings/app:<slug>) is not a grant. -->
    {#if tab.startsWith('app:') && appTabs.some((t) => t.id === tab)}<AppSurface slug={tab.slice(4)} surface="settings" />{/if}
    </div>
    {/key}
</SettingsPage>
