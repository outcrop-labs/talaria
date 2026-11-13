<script lang="ts">
  import type { Snippet } from 'svelte'
  import type { HTMLAttributes } from 'svelte/elements'
  import Panel from '@/components/ui/Panel.svelte'
  import { cn } from '@/lib/cn'

  // The settings card: rows in one panel, divided by hairlines. Children are
  // `SettingsRow`s or any row-shaped content. Never nest one in another.
  let { class: className, children, ...rest }: HTMLAttributes<HTMLElement> & { class?: string; children: Snippet } = $props()
</script>

<Panel class={cn('settings-group overflow-hidden p-0 [&>*+*]:border-t [&>*+*]:border-line-subtle', className)} {...rest}>
  {@render children()}
</Panel>

<style>
  /* The card is a two-column grid and every row borrows its columns, so the
     right side is as wide as the widest row needs and all rows share it: a
     status starts at its left edge, a control ends at its right. A wrapper
     around a row (a data hook, a transition) passes the columns through.
     Below sm, and in an engine without subgrid, rows are the flex rows they
     are anywhere else. */
  @media (min-width: 640px) {
    @supports (grid-template-columns: subgrid) {
      :global(.settings-group) {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
      }
      :global(.settings-group > *),
      :global(.settings-group :has([data-settings-row]):not([hidden]) > *) {
        grid-column: 1 / -1;
        min-width: 0;
      }
      :global(.settings-group [data-settings-row]),
      :global(.settings-group :has([data-settings-row]):not([hidden])) {
        display: grid;
        grid-template-columns: subgrid;
      }
      :global(.settings-group [data-settings-row] > [data-row-controls]) {
        grid-column: 2;
      }
      :global(.settings-group [data-settings-row] > :only-child),
      :global(.settings-group [data-settings-row='stacked'] > *) {
        grid-column: 1 / -1;
      }
    }
  }
</style>
