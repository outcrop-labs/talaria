<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { AgentContainers, AgentDef } from '@/lib/fleet-defs'
  import { HEALTH_COLOR, healthOf } from './agents.svelte'

  let { def: d, containers }: { def: AgentDef; containers: AgentContainers | null } = $props()

  const health = $derived(healthOf(d, containers).health)
</script>

<!-- Spec §8 status dot: 6–7px round, signal color carries the meaning. The
     health hues are data (five fleet states, not five tones), so they ride
     `color`; the 7px size is this site's own. -->
<StatusDot
  color={HEALTH_COLOR[health]}
  pulse={health === 'warming'}
  title={health === 'warming' ? 'warming up' : health}
  class="h-[7px] w-[7px]"
/>
