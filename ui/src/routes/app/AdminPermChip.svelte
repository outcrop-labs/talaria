<script lang="ts">
  import { Check } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import type { PermCatalogEntry } from './admin'

  /** One permission as a toggle chip. Filled = effectively allowed. A dot marks
   *  a value overridden away from the org default; clicking toggles, and
   *  toggling back to the default clears the override. */
  let {
    entry,
    effective,
    overridden,
    onToggle,
  }: {
    entry: PermCatalogEntry
    effective: boolean
    overridden: boolean
    onToggle: () => void
  } = $props()
</script>

<button
  type="button"
  aria-pressed={effective}
  title={`${entry.hint}${overridden ? ' (Changed from the default. Select it again to go back.)' : ''}`}
  onclick={onToggle}
  class={cn(
    'inline-flex h-7 items-center gap-1.5 rounded-md border px-2 font-sans text-xs font-medium leading-4 transition-colors',
    focusGold,
    effective
      ? 'border-accent/40 bg-accent/12 text-fg'
      : 'border-line text-muted hover:border-line-strong hover:text-fg',
  )}
>
  {#if effective}<Check size={12} class="text-accent" aria-hidden="true" />{/if}
  {entry.label}
  {#if overridden}<span class="h-1.5 w-1.5 rounded-full bg-warning" title="Changed from the default"></span>{/if}
</button>
