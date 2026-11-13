<script lang="ts">
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import { fly } from '@/lib/motion'
  import { tabFromPath } from '@/lib/route-tabs'
  import { useSession } from '@/lib/session'
  import { APPS_TABS } from '@/lib/settings-nav'
  import { navigate, route } from '@/router'
  import AppsInstalledTab from './AppsInstalledTab.svelte'
  import AppsDiscoverTab from './AppsDiscoverTab.svelte'

  // Apps: what this deployment has installed, and the catalogs to install
  // more from. Installing clones the app's codebase into the deployment: dev
  // picks it up live, prod needs a rebuild.
  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  // The URL is the tab: /apps (installed) and /apps/discover.
  type AppsTab = (typeof APPS_TABS)[number]['id']
  const tab = $derived(tabFromPath(route.pathname, '/apps', APPS_TABS.map((t) => t.id), 'installed'))
  const goTab = (id: AppsTab) => {
    if (id === 'installed') void navigate('/apps')
    else void navigate('/apps/:tab', { params: { tab: id } })
  }
</script>

<SettingsPage
  title="Apps"
  blurb="Add work and manage views built for your team."
  tabs={APPS_TABS}
  {tab}
  onTab={goTab}
  staggerItems="[data-app-cards] > *"
>
  <!-- Tab-pane grammar: rise in on switch, no exit. On mount the app rows and
       cards (each tab's [data-app-cards] hook) cascade. The selector
       deliberately misses the skeleton rows. -->
  {#key tab}
    <div in:fly={{ y: 6, duration: 200 }} class="space-y-10" data-stagger-items="[data-app-cards] > *">
      {#if tab === 'installed'}<AppsInstalledTab {isAdmin} />{:else}<AppsDiscoverTab {isAdmin} />{/if}
    </div>
  {/key}
</SettingsPage>
