<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { AgentContainers, AgentDef } from '@/lib/fleet-defs'
  import { HEALTH_COLOR, healthOf } from './agents.svelte'

  let { def: d, containers }: { def: AgentDef; containers: AgentContainers | null } = $props()

  const health = $derived(healthOf(d, containers).health)
</script>

<!-- Spec §8 status dot: 6–7px round, signal color carries the meaning. -->
<span
  class={cn('h-[7px] w-[7px] shrink-0 rounded-full', health === 'warming' && 'gd-breathe')}
  style:background={HEALTH_COLOR[health]}
  title={health === 'warming' ? 'warming up' : health}
></span>
