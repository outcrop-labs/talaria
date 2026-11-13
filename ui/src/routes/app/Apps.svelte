<script lang="ts">
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import { fly } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import AppsInstalledTab from './AppsInstalledTab.svelte'
  import AppsDiscoverTab from './AppsDiscoverTab.svelte'

  // Manage → Apps: the platform's app system in one place. Installed = what
  // this deployment ships (enable/disable, uninstall); Discover = the
  // marketplace — community + official apps from the catalog feed, plus
  // install-from-git for anything else. Installing clones the app's codebase
  // into the deployment: dev picks it up live, prod needs a rebuild.
  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  let tab = $state<'installed' | 'discover'>('installed')
</script>

<SettingsPage
  title="Apps"
  blurb="Add work and manage views built for your team."
  tabs={[
    { id: 'installed', label: 'Installed' },
    { id: 'discover', label: 'Discover' },
  ]}
  {tab}
  onTab={(id) => (tab = id)}
  staggerItems="[data-app-cards] > *"
>

    <!-- Tab-pane grammar: rise in on switch, no exit; the page scrolls, so no
         AutoHeight. On MOUNT the pane is a section whose items cascade: the
         app cards (each tab's [data-app-cards] hook) rise 30ms apart while
         the frame fades. The selector deliberately misses the skeleton rows. -->
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} data-stagger-items="[data-app-cards] > *">
        {#if tab === 'installed'}<AppsInstalledTab {isAdmin} />{:else}<AppsDiscoverTab {isAdmin} />{/if}
      </div>
    {/key}
</SettingsPage>
