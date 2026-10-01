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

  // No view-title claim from the console, and nothing would read one anyway
  // (lib/view-title.svelte.ts has no reader since the strip was deleted). A
  // mount-time greeting claim was tried and rotted even while the strip
  // existed: tab switches change the path without remounting, so the claim's
  // key went stale and every tab after the first fell back to "Inbox".
</script>

<PageSurface>
  <!-- Page content entrance: tab strip → pane rise in sequence (ANIMATIONS.md).
       The keyed pane below keeps its own fly on tab switch — one level of
       stagger only, so the dense panes themselves stay flat. -->
  <div use:staggerIn class="space-y-6">
    <HomeTabs value={tab} />

    <!-- The inbox tab is the daily brief now (Home.svelte renders
         `DailyBrief` for it; this console never mounts for that tab), which
         is why the old inbox stack — briefing, notifications, approvals — is
         gone from here. `home` goes into BoardsTab whole, not pre-unwrapped:
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
