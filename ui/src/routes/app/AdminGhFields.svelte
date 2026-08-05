<script lang="ts">
  /** Field-by-field form guidance: GitHub's field name → exactly what to enter. */
  let { rows }: { rows: Array<[string, string]> } = $props()
</script>

<div class="divide-y divide-line-subtle rounded-md border border-line bg-raised/40">
  {#each rows as [label, value] (label)}
    <div class="flex items-baseline gap-3 px-3 py-1.5">
      <span class="w-52 shrink-0 text-xs text-muted">{label}</span>
      <span class="min-w-0 flex-1 text-xs text-fg">{value}</span>
    </div>
  {/each}
</div>
