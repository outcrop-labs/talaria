<script lang="ts">
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Tabs from '@/components/ui/Tabs.svelte'
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

<div class="h-full overflow-y-auto p-8">
  <div class="mx-auto w-full max-w-4xl">
    <div class="mb-1 flex items-baseline gap-3">
      <h1 class="font-sans text-2xl font-semibold tracking-tight text-fg">Apps</h1>
      <InfoTip text="Self-contained apps that compile into this deployment and render as native Talaria surfaces — built by your team, the community, or Outcrop. They run under each signed-in user's session, so platform permissions apply unchanged." />
    </div>
    <p class="mb-6 font-sans text-xs text-muted">
      Extend Talaria with new work and manage views. Apps live in the <code class="text-fg">apps/</code> directory and become part of the deployment.
    </p>

    <Tabs
      class="mb-6"
      items={[
        { id: 'installed', label: 'Installed' },
        { id: 'discover', label: 'Discover' },
      ]}
      value={tab}
      onChange={(id) => (tab = id)}
    />

    {#if tab === 'installed'}<AppsInstalledTab {isAdmin} />{:else}<AppsDiscoverTab {isAdmin} />{/if}
  </div>
</div>
