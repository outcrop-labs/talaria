<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { AgentBrainHealth } from '@/lib/fleet-defs'

  /** The agent's configured model lost its gateway route (provider-pool
   *  churn). The main model is a failure; a tier or fallback is a warning.
   *  A state, so it is a word in the tone's ink beside the card's status,
   *  not a chip. */
  let { brain, class: className }: { brain?: AgentBrainHealth; class?: string } = $props()

  const bad = $derived(brain ? brain.targets.filter((t) => !t.ok) : [])
  const detail = $derived(
    bad.map((t) => `${t.kind}${t.name ? ` "${t.name}"` : ''} → ${t.endpoint}/${t.model}: ${t.reason}`).join('\n'),
  )
</script>

{#if brain && bad.length > 0}
  <span title={detail} class={cn('font-sans text-[13px] leading-5', brain.ok ? 'text-warning' : 'text-danger', className)}>
    {brain.ok ? `${bad.length} model route${bad.length === 1 ? '' : 's'} unreachable` : 'Model unreachable'}
  </span>
{/if}
