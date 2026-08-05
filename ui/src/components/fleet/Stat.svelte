<script lang="ts">
  // §8 stat cell: 10px mono uppercase ink-dim label over a readout value.
  let { label, value }: { label: string; value: string } = $props()
</script>

<div>
  <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</div>
  <div class="truncate text-fg" title={value}>{value}</div>
</div>
