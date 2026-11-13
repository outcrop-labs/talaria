<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import type { AgentBrainHealth } from '@/lib/fleet-defs'

  /** The agent's configured model lost its gateway route (provider-pool
   *  churn). The main model is a failure; a tier or fallback is a warning. */
  let { brain, class: className }: { brain?: AgentBrainHealth; class?: string } = $props()

  const bad = $derived(brain ? brain.targets.filter((t) => !t.ok) : [])
  const detail = $derived(
    bad.map((t) => `${t.kind}${t.name ? ` "${t.name}"` : ''} → ${t.endpoint}/${t.model}: ${t.reason}`).join('\n'),
  )
</script>

{#if brain && bad.length > 0}
  <Chip tone={brain.ok ? 'warn' : 'danger'} title={detail} class={className}>
    {brain.ok ? `${bad.length} model route${bad.length === 1 ? '' : 's'} down` : 'Model unreachable'}
  </Chip>
{/if}
