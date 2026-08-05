<script lang="ts">
  import { cn } from '@/lib/cn'
  import WingMark from './WingMark.svelte'

  // The Talaria brand — gold "A"-delta mark + spaced-caps wordmark
  // (Mercury spec §4, exact SVG from the design file).
  let { class: className, showTag = false }: { class?: string; showTag?: boolean } = $props()
</script>

<div class={cn('flex items-center gap-2.5', className)}>
  <WingMark class="h-5 w-5" />
  <div class="leading-tight">
    <div class="font-sans text-[13px] leading-4 font-semibold tracking-[0.18em] text-fg">
      TALARIA
    </div>
    {#if showTag}
      <div class="theme-muted text-[11px] tracking-wide">the winged fleet cockpit</div>
    {/if}
  </div>
</div>
