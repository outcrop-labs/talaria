<script lang="ts">
  /** Field-by-field form guidance: GitHub's field name → exactly what to enter. */
  let { rows }: { rows: Array<[string, string]> } = $props()
</script>

<dl class="divide-y divide-line-subtle rounded-md border border-line">
  {#each rows as [label, value] (label)}
    <div class="grid grid-cols-[12rem_minmax(0,1fr)] gap-x-4 px-3 py-2">
      <dt class="font-sans text-[13px] leading-5 text-muted">{label}</dt>
      <dd class="font-sans text-[13px] leading-5 text-fg">{value}</dd>
    </div>
  {/each}
</dl>
