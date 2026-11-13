<script lang="ts">
  import { cn } from '@/lib/cn'
  import FitnessMark from './FitnessMark.svelte'
  import { BAND_META, type FitnessBand } from './fitness'

  // What each mark in the matrix means. Not decoration: Untested and No harness
  // are the two an admin is most likely to misread as a pass, and they are the
  // two that mean the opposite.
  let { class: className }: { class?: string } = $props()

  const BANDS = Object.keys(BAND_META) as FitnessBand[]
</script>

<div class={cn('flex flex-wrap items-center gap-x-5 gap-y-1', className)}>
  {#each BANDS as band (band)}
    <span class="flex items-center gap-2 font-sans text-xs leading-4 text-muted" title={BAND_META[band].blurb}>
      <span class="grid h-4 w-3 place-items-center"><FitnessMark {band} /></span>
      {BAND_META[band].label}
    </span>
  {/each}
</div>
