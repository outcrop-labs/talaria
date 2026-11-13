<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { AgentContainers, AgentDef } from '@/lib/fleet-defs'
  import { HEALTH_COLOR, HEALTH_LABEL, healthOf } from './agents.svelte'

  let { def: d, containers }: { def: AgentDef; containers: AgentContainers | null } = $props()

  const health = $derived(healthOf(d, containers).health)
</script>

<!-- The health hues are data (six fleet states, not five tones), so they ride
     `color`. A draft is not a fault: the title says what it is. -->
<span
  class="flex shrink-0 items-center gap-2 font-sans text-[13px] leading-5 text-muted"
  title={health === 'draft' ? 'Hired without a container. Start it to finish the hire.' : undefined}
>
  <StatusDot color={HEALTH_COLOR[health]} pulse={health === 'warming'} />
  {HEALTH_LABEL[health]}
</span>
