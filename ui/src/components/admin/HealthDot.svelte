<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'

  let { ok, label }: { ok: boolean; label: string } = $props()
</script>

<span class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
  <StatusDot status={ok ? 'ok' : 'danger'} />
  {label}
  {ok ? 'up' : 'down'}
</span>
