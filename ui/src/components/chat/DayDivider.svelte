<script lang="ts">
  import { cn } from '@/lib/cn'

  // The transcript's day divider (R1): a hairline rule with a centered pill in
  // the raised-tile style (spec §8) — the label comes from lib/day-dividers.
  let { label, class: className }: { label: string; class?: string } = $props()
</script>

<div role="separator" aria-label={label} class={cn('relative flex items-center py-1', className)}>
  <div class="h-px flex-1 bg-line"></div>
  <span
    class="mx-2 shrink-0 rounded-full border border-line bg-raised px-3 py-0.5 font-sans text-[11px] font-medium text-muted"
  >
    {label}
  </span>
  <div class="h-px flex-1 bg-line"></div>
</div>
