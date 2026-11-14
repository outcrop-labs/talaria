<script lang="ts">
  // One part of a whole, as a bar on a full-width track with its percentage
  // at the right, in the colour of the series it belongs to.
  let { share, color }: { share: number; color: string } = $props()

  const percent = $derived(Math.min(100, Math.max(0, share * 100)))
  const text = $derived(percent > 0 && percent < 1 ? '<1%' : `${Math.round(percent)}%`)
</script>

<div class="flex items-center gap-3">
  <div class="h-1.5 min-w-16 flex-1 overflow-hidden rounded-full bg-line" aria-hidden="true">
    <div class="h-full rounded-full" style:width="{percent}%" style:background={color}></div>
  </div>
  <span class="w-9 shrink-0 text-right font-sans text-[13px] tabular-nums leading-5 text-muted">{text}</span>
</div>
