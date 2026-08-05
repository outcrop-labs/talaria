<script lang="ts">
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
  title={`${entry.hint}${overridden ? ' (overridden — click twice to return to default)' : ''}`}
  onclick={onToggle}
  class={cn(
    'flex items-center gap-1 rounded-md border px-2 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
    focusGold,
    effective
      ? 'border-accent/60 bg-accent/10 text-fg'
      : 'border-line-subtle text-muted hover:border-line hover:text-fg',
  )}
>
  {entry.label}
  {#if overridden}<span class="h-1 w-1 rounded-full bg-warning" title="overridden"></span>{/if}
</button>
