<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { AgentContainers, AgentDef } from '@/lib/fleet-defs'
  import { HEALTH_COLOR, healthOf } from './agents.svelte'

  let { def: d, containers }: { def: AgentDef; containers: AgentContainers | null } = $props()

  const health = $derived(healthOf(d, containers).health)
</script>
<!-- Spec §8 status dot: 6–7px round, signal color carries the meaning. The
     health hues are data (six fleet states, not six tones), so they ride
     `color`; the 7px size is this site's own. 'draft' is not a fault — the
     tooltip says what it is instead of letting a neutral dot read as
     "unknown". -->
<StatusDot
  color={HEALTH_COLOR[health]}
  pulse={health === 'warming'}
  title={health === 'warming' ? 'warming up' : health === 'draft' ? 'draft — hired without a container; start it to finish the hire' : health}
  class="h-[7px] w-[7px]"
/>
