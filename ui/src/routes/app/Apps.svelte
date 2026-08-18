<script lang="ts">
  import PageSurface from '@/components/app/PageSurface.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
  import ViewHeader from '@/components/ui/ViewHeader.svelte'
  import { fly, staggerIn } from '@/lib/motion'
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

<PageSurface>
  <!-- Page content entrance: title → blurb → tab strip → pane rise in sequence
       (ANIMATIONS.md). The keyed pane keeps its own fly on tab switch and
       stays unstaggered inside — one level only. -->
  <div use:staggerIn>
    <ViewHeader
      class="mb-6"
      title="Apps"
      info="Self-contained apps that compile into this deployment and render as native Talaria surfaces — built by your team, the community, or Outcrop. They run under each signed-in user's session, so platform permissions apply unchanged."
    >
      {#snippet blurb()}
        Extend Talaria with new work and manage views. Apps live in the <code class="text-fg">apps/</code> directory and become part of the deployment.
      {/snippet}
    </ViewHeader>

    <Tabs
      class="mb-6"
      items={[
        { id: 'installed', label: 'Installed' },
        { id: 'discover', label: 'Discover' },
      ]}
      value={tab}
      onChange={(id) => (tab = id)}
    />

    <!-- Tab-pane grammar: rise in on switch, no exit; the page scrolls, so no
         AutoHeight. On MOUNT the pane is a section whose items cascade: the
         app cards (each tab's [data-app-cards] hook) rise 30ms apart while
         the frame fades. The selector deliberately misses the skeleton rows. -->
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }} data-stagger-items="[data-app-cards] > *">
        {#if tab === 'installed'}<AppsInstalledTab {isAdmin} />{:else}<AppsDiscoverTab {isAdmin} />{/if}
      </div>
    {/key}
  </div>
</PageSurface>
