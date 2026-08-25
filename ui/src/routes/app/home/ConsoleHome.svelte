<script lang="ts">
  import PageSurface from '@/components/app/PageSurface.svelte'
  import { fly, staggerIn } from '@/lib/motion'
  import { useHome, type HomeTab } from './home'
  import HomeTabs from './HomeTabs.svelte'
  import BoardsTab from './BoardsTab.svelte'
  import CommsTab from './CommsTab.svelte'
  import PlansTab from './PlansTab.svelte'
  import ResearchTab from './ResearchTab.svelte'
  import DocsTab from './DocsTab.svelte'

  let { tab }: { tab: Exclude<HomeTab, 'inbox'> } = $props()

  const home = useHome()

  // No strip claim from the console: the strip's route fallback names each
  // tab by its segment (TopStrip's viewName — "Boards", "Comms", …), which is
  // the honest title for a tabbed digest and needs no per-tab wiring here. A
  // mount-time greeting claim was tried and rotted: tab switches change the
  // path without remounting, so the claim's key went stale and every tab
  // after the first fell back to "Inbox".
</script>

<PageSurface>
  <!-- Page content entrance: tab strip → pane rise in sequence (ANIMATIONS.md).
       The keyed pane below keeps its own fly on tab switch — one level of
       stagger only, so the dense panes themselves stay flat. -->
  <div use:staggerIn class="space-y-6">
    <HomeTabs value={tab} />

    <!-- The inbox tab is the focus queue now (Home.svelte renders
         <FocusInbox> before this console ever mounts), which is why the
         old inbox stack — briefing, notifications, approvals — is gone
         from here. `home` goes into BoardsTab whole, not pre-unwrapped:
         the tab needs `isError`/`refetch` to tell an empty queue from a
         queue it could not read. -->
    <!-- Tab-pane grammar: rise in on switch, no exit. These are dense work
         queues (boards, comms) — no stagger, no AutoHeight. -->
    {#key tab}
      <div in:fly={{ y: 6, duration: 200 }}>
        {#if tab === 'boards'}<BoardsTab {home} />{/if}
        {#if tab === 'comms'}<CommsTab />{/if}
        {#if tab === 'plans'}<PlansTab />{/if}
        {#if tab === 'research'}<ResearchTab />{/if}
        {#if tab === 'docs'}<DocsTab />{/if}
      </div>
    {/key}
  </div>
</PageSurface>
