<script lang="ts">
  import { cn } from '@/lib/cn'

  let { ok, label }: { ok: boolean; label: string } = $props()
</script>

<span class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
  <span class={cn('h-1.5 w-1.5 rounded-full', ok ? 'bg-success' : 'bg-danger')}></span>
  {label}
  {ok ? 'up' : 'down'}
</span>
