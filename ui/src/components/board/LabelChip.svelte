<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { BoardLabel } from '@/lib/boards.svelte'
  import { labelColor } from './field-pills'

  // The one tinted label chip — used on cards, rows, facets, and settings.
  // Mono chip voice (spec §8).
  let { name, labels, class: className }: { name: string; labels: BoardLabel[]; class?: string } = $props()

  const c = $derived(labelColor(name, labels))
</script>

<span
  class={cn('max-w-28 truncate rounded-full border px-1.5 py-0.5 font-mono text-[10px] normal-case tracking-[0.05em]', className)}
  style:color={c}
  style:border-color="color-mix(in srgb, {c} 45%, transparent)"
  style:background="color-mix(in srgb, {c} 10%, transparent)"
>
  {name}
</span>
