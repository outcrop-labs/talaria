<script lang="ts">
  import Panel from '@/components/ui/Panel.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'

  // Report-document placeholder: a panel of prose bars, shaped like the
  // Markdown report it stands in for.
  const widths = ['66%', '100%', '94%', '88%', '97%', '82%', '91%', '100%', '86%', '95%', '78%', '58%']
</script>

<Panel aria-hidden="true">
  <div class="space-y-3">
    {#each widths as w, i (i)}
      <div style:width={w}>
        <Skeleton class={i === 0 ? 'h-4 w-full rounded-full' : 'h-2.5 w-full rounded-full'} />
      </div>
    {/each}
  </div>
</Panel>
