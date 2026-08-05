<script lang="ts" generics="T extends string">
  import { cn } from '@/lib/cn'
  import { focusRing } from './control'
  import type { SegmentedOption } from './segmented'

  // The one segmented control — a bordered group of exclusive options. Use for
  // small mode switches (read/edit, list/grid); Tabs for page-level sections.
  // Mercury: the AUTO/MANUAL mono toggle from board `8A-0` — hairline group,
  // active cell raised + readout, inactive cells muted, mono uppercase labels.
  let {
    options,
    value,
    onChange,
    size = 'sm',
    class: className,
  }: {
    options: ReadonlyArray<SegmentedOption<T>>
    value: T
    onChange: (id: T) => void
    size?: 'xs' | 'sm'
    class?: string
  } = $props()
</script>

<div class={cn('inline-flex shrink-0 rounded-md border border-line p-0.5', className)}>
  {#each options as o (o.id)}
    <button
      type="button"
      title={o.title}
      aria-pressed={value === o.id}
      onclick={() => onChange(o.id)}
      class={cn(
        'rounded font-mono uppercase tracking-[0.05em] transition-colors',
        focusRing,
        size === 'xs' ? 'px-2 py-0.5 text-[10px] leading-3' : 'px-2.5 py-1 text-[10px] leading-4',
        value === o.id ? 'bg-raised text-fg' : 'text-muted hover:text-fg',
      )}
    >
      {#if typeof o.label === 'string'}{o.label}{:else}{@render o.label()}{/if}
    </button>
  {/each}
</div>
