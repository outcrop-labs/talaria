<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { AgentBrainHealth } from '@/lib/fleet-defs'

  /** "brain unroutable" chip — the agent's configured model lost its gateway
   *  route (provider-pool churn). Main = red, tier/fallback only = amber. */
  let { brain }: { brain?: AgentBrainHealth } = $props()

  const bad = $derived(brain ? brain.targets.filter((t) => !t.ok) : [])
  const detail = $derived(
    bad.map((t) => `${t.kind}${t.name ? ` "${t.name}"` : ''} → ${t.endpoint}/${t.model}: ${t.reason}`).join('\n'),
  )
</script>

{#if brain && bad.length > 0}
  <!-- Signal reads as an OUTLINE chip (spec §8: orange is never a fill). -->
  <span
    title={detail}
    class={cn(
      'shrink-0 rounded border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.05em]',
      !brain.ok ? 'border-danger/40 text-danger' : 'border-warning/40 text-warning',
    )}
  >
    {!brain.ok ? 'brain unroutable' : `${bad.length} tier${bad.length === 1 ? '' : 's'} down`}
  </span>
{/if}
