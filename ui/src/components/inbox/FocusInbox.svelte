<script lang="ts">
  import HomeTabs from '@/routes/app/home/HomeTabs.svelte'
  import type { Snippet } from 'svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { staggerIn } from '@/lib/motion'
  import { useInboxFocusWorkspace } from './inbox-focus-shell'
  import { isEditable } from './focus-inbox'
  import FocusCard from './FocusCard.svelte'
  import FocusHeader from './FocusHeader.svelte'
  import FocusLoading from './FocusLoading.svelte'
  import FocusOffline from './FocusOffline.svelte'
  import InboxZero from './InboxZero.svelte'
  import QueuePreview from './QueuePreview.svelte'
  import UtilityDrawer from './UtilityDrawer.svelte'

  let { mail, agenda }: { mail: Snippet; agenda: Snippet } = $props()

  let drawer = $state<'mail' | 'agenda' | null>(null)
  // Never destructure the workspace — its fields are reactive getters.
  const workspace = useInboxFocusWorkspace()

  function onKey(event: KeyboardEvent) {
    const active = workspace.active
    if (!active || workspace.busyAction !== null || isEditable(event.target) || event.metaKey || event.ctrlKey || event.altKey) return
    const key = event.key.toLowerCase()
    if (key === 'a') {
      event.preventDefault()
      if (workspace.recommendedAction) void workspace.performAction(active, workspace.recommendedAction.id)
      else window.location.assign(active.sourceHref)
    }
    if (key === 'o') {
      event.preventDefault()
      window.location.assign(active.sourceHref)
    }
    if (key === 's') {
      event.preventDefault()
      void workspace.snooze()
    }
  }

  // THE FAILURE MODES THIS SPLIT EXISTS FOR.
  //
  // This is the HITL surface: everything blocked on a person arrives here. The
  // one thing it must never do is render "nothing needs you" over a read it
  // could not make — the exact bug `WorkbenchJobsStrip` had on the ticket,
  // where a 500 erased the approval gate and the ticket looked idle while an
  // agent sat stopped behind it.
  //
  // `isError` alone was half the answer, and it was wrong in BOTH directions:
  //
  //   · A failed BACKGROUND refetch (svelte-query keeps `data` and flips
  //     `isError`) replaced a live queue of real decisions with a full-page
  //     "Queue offline". Actionable items — an approval an agent is waiting on
  //     — were thrown away because a later poll blipped. Stale beats blank, as
  //     long as it says it is stale.
  //   · The header read `data?.counts.total ?? 0` regardless, so a failure that
  //     DID take the page still printed a confident "Inbox · 0 / 00 / 00" at
  //     the top. A person who glances at that number and walks away has been
  //     told the queue is empty by the surface that could not read it.
  const failed = $derived(workspace.isError && workspace.data === undefined)
  const stale = $derived(workspace.isError && workspace.data !== undefined)
</script>

<svelte:document onkeydown={onKey} />

<div class="h-full min-h-0 min-w-0 overflow-y-auto px-4 pb-8 pt-8 sm:px-8 sm:pt-12">
  <!-- Page content entrance: header then the queue block rise in sequence
       (ANIMATIONS.md). Runs once at mount; the loaded branch below staggers
       again when data replaces the skeleton. -->
  <main use:staggerIn class="mx-auto w-full max-w-[var(--page-width)]">
    <!-- Home's tab strip. It lived in ConsoleHome, which never renders for this
         tab, so the surface most people land on had no way out of itself. -->
    <div class="mb-5"><HomeTabs value="inbox" /></div>
    <FocusHeader
      count={failed ? null : (workspace.data?.counts.total ?? 0)}
      current={workspace.active ? 1 : 0}
      onOpenMail={() => (drawer = 'mail')}
      onOpenAgenda={() => (drawer = 'agenda')}
    />

    {#if stale}
      <QueryError
        variant="inline"
        class="mt-4 rounded-lg border border-[color:var(--theme-danger)]/40 bg-[color:var(--theme-danger)]/5 px-4 py-2.5"
        error={workspace.error}
        title="This queue may be out of date; the last refresh failed"
        onRetry={() => void workspace.refetch()}
      />
    {/if}

    {#if failed}
      <FocusOffline error={workspace.error} onRetry={() => void workspace.refetch()} />
    {:else}
      <!-- `workspace.data === undefined` (not `isLoading`): svelte-query's
           "first fetch IN FLIGHT" is false in the states between — a disabled
           query, a retry backoff, a mount before the fetch starts. Every one
           of those has no data and no error, and branching on `isLoading`
           alone dropped them through to `InboxZero`: "no decisions are
           waiting", asserted by a surface that had not asked yet. The
           skeleton is the honest placeholder for "we do not know".

           Skeleton → content as one motion: FocusLoading sketches the card +
           queue silhouette ONCE (count=1 — this surface is a card plus a
           short queue, not a homogeneous list), and the resolved card and
           queue preview stagger in over it (Materialize's content branch
           owns the cascade — the old staggerIn/data-stagger-items pair is
           retired with it). -->
      <Materialize loading={workspace.data === undefined} count={1}>
        {#snippet skeleton()}<FocusLoading />{/snippet}
        {#if workspace.active}
          <FocusCard
            item={workspace.active}
            recommendedAction={workspace.recommendedAction}
            busyAction={workspace.busyAction}
            snoozeMs={workspace.snoozeMs}
            onSnoozeMs={workspace.setSnoozeMs}
            onAction={(action) => void workspace.performAction(workspace.active!, action.id)}
            onSnooze={() => void workspace.snooze()}
            onSkip={workspace.skip}
            canSkip={workspace.orderedItems.length > 1}
          />
          <QueuePreview items={workspace.orderedItems.slice(1, 5)} remaining={Math.max(0, workspace.orderedItems.length - 1)} />
        {:else}
          <!-- Only reachable with data in hand, so "inbox zero" is something
               the server actually said — never a stand-in for a read that
               failed or a read that has not happened. -->
          <InboxZero />
        {/if}
      </Materialize>
    {/if}
  </main>
</div>

{#if drawer}
  <UtilityDrawer title={drawer === 'mail' ? 'Mail' : 'Agenda'} onClose={() => (drawer = null)}>
    {#if drawer === 'mail'}{@render mail()}{:else}{@render agenda()}{/if}
  </UtilityDrawer>
{/if}
