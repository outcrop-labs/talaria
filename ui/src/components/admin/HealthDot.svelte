<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'

  // `ok: null` is "not probed" — nothing to test from, which is neither up
  // nor down, so it gets the idle dot instead of a red one.
  let { ok, label, title }: { ok: boolean | null; label: string; title?: string } = $props()
</script>

<span {title} class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
  <StatusDot status={ok === null ? 'idle' : ok ? 'ok' : 'danger'} />
  {label}
  {ok === null ? 'not probed' : ok ? 'up' : 'down'}
</span>
