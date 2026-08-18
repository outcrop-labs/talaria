<script lang="ts">
  import PageSurface from '@/components/app/PageSurface.svelte'
  import ViewHeader from '@/components/ui/ViewHeader.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import { useSession } from '@/lib/session'
  import { greeting, useHome, type HomeTab } from './home'
  import HomeTabs from './HomeTabs.svelte'
  import BoardsTab from './BoardsTab.svelte'
  import CommsTab from './CommsTab.svelte'
  import PlansTab from './PlansTab.svelte'
  import ResearchTab from './ResearchTab.svelte'
  import DocsTab from './DocsTab.svelte'
  import FleetTab from './FleetTab.svelte'

  let { tab }: { tab: Exclude<HomeTab, 'inbox'> } = $props()

  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const home = useHome()


</script>

<PageSurface>
  <!-- Page content entrance: greeting → tab strip → pane rise in sequence
       (ANIMATIONS.md). The keyed pane below keeps its own fly on tab switch —
       one level of stagger only, so the dense panes themselves stay flat. -->
  <div use:staggerIn class="space-y-6">
    <ViewHeader title={greeting(session.data?.name ?? session.data?.email)} />

    <HomeTabs value={tab} />

    <!-- The inbox tab is the focus queue now (Home.svelte renders
         <FocusInbox> before this console ever mounts), which is why the
         old inbox stack — briefing, notifications, approvals — is gone
         from here. `home` goes into BoardsTab whole, not pre-unwrapped:
         the tab needs `isError`/`refetch` to tell an empty queue from a
         queue it could not read. -->
    <!-- Tab-pane grammar: rise in on switch, no exit. These are dense work
         queues (boards, comms, fleet) — no stagger, no AutoHeight. -->
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }}>
        {#if tab === 'boards'}<BoardsTab {home} />{/if}
        {#if tab === 'comms'}<CommsTab />{/if}
        {#if tab === 'plans'}<PlansTab />{/if}
        {#if tab === 'research'}<ResearchTab />{/if}
        {#if tab === 'docs'}<DocsTab />{/if}
        {#if tab === 'fleet' && isAdmin}<FleetTab {home} />{/if}
      </div>
    {/key}
  </div>
</PageSurface>
