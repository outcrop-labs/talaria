<script lang="ts" module>
  import type { Snippet } from 'svelte'
  import type { ContextMenuController } from '@/components/ui/context-menu.svelte'
  import type { QueryErrorProps } from '@/components/ui/query-state'

  /** WHAT EVERY HOME TAB'S LIST PANEL IS.
   *
   *  Docs, Plans, Comms and Research each had their own copy of the same branch
   *  chain, and the chain is the part with the RULES in it: the notice renders
   *  ABOVE the list (a stale read keeps its rows and wears the marker), `failed`
   *  renders nothing beneath it (the notice already said the list is missing —
   *  an empty list beside an error reads as "and also there are none"), and the
   *  empty state is the RESOLVED-empty answer only, never the loading or broken
   *  one. Four copies of that is four chances to drop one.
   *
   *  Everything past the branch is the caller's: heading text, the zero state's
   *  wording, the rows, and the query itself. Row markup stays per-tab because
   *  the rows genuinely differ — a chip and a timestamp here, an unread count
   *  there — and a `row` snippet with six variants would be this component
   *  pretending to be four. */
  export interface HomeListPanelProps {
    /** The panel heading (SectionHeader). */
    title: string
    /** Right-aligned mono meta in the heading — Comms' unread count. */
    action?: string
    /** `listQuery(...).notice`. Passed IN rather than read off a query here, so
     *  the call site still spells `list.notice`: the `listquery-notice-dropped`
     *  invariant scans the CALLER file for that literal, and a shell that took
     *  the whole `list` would launder the failure the moment it was bound. */
    notice: QueryErrorProps | null
    /** `listQuery(...).pending` — first load, skeleton instead of a zero state. */
    pending: boolean
    /** `listQuery(...).failed` — broke with nothing behind it. */
    failed: boolean
    /** Resolved, and this surface has nothing to show. The caller computes it
     *  because the tab may be listing a SUBSET of its rows (Docs' 12 most recent,
     *  Comms' unread) — those can be empty over a read that returned rows. */
    isEmpty: boolean
    /** Skeleton row count — the rows are tab-shaped, so the count is too. */
    skeletonRows: number
    /** The tab's own `useContextMenu()`. Rows stay the caller's, so the caller
     *  is the one that opens the menu; this only renders it (last, so the portal
     *  is outside every panel). */
    menu: ContextMenuController
    /** The tab's own zero state. */
    empty: Snippet
    /** A sibling BELOW the panel, inside the tab's stack — Comms' activity feed. */
    after?: Snippet
    /** The `<li>` rows. The `<ul>` (and its separators) is this component's. */
    children: Snippet
  }
</script>

<script lang="ts">
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'

  let {
    title,
    action,
    notice,
    pending,
    failed,
    isEmpty,
    skeletonRows,
    menu,
    empty,
    after,
    children,
  }: HomeListPanelProps = $props()
</script>

<div class="space-y-6">
  <Panel>
    <SectionHeader {title} {action} />
    {#if notice}
      <QueryError {...notice} />
    {/if}
    {#if pending}
      <SkeletonRows rows={skeletonRows} />
    {:else if failed}
      <!-- the notice above already covers it -->
    {:else if isEmpty}
      {@render empty()}
    {:else}
      <ul class="divide-y divide-line">
        {@render children()}
      </ul>
    {/if}
  </Panel>
  {#if after}{@render after()}{/if}
  <ContextMenu {menu} />
</div>