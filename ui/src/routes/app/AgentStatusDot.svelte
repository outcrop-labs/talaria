<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { AgentBrainHealth, AgentContainers, AgentDef } from '@/lib/fleet-defs'
  import { HEALTH_LABEL, HEALTH_TONE, shownHealth } from './agents.svelte'

  let { def: d, containers, brain }: { def: AgentDef; containers: AgentContainers | null; brain?: AgentBrainHealth } = $props()

  const health = $derived(shownHealth(d, containers, brain))
</script>

<!-- A draft is not a fault: the title says what it is. -->
<span
  class="flex shrink-0 items-center gap-2 font-sans text-[13px] leading-5 text-muted"
  title={health === 'draft' ? 'Hired without a container. Start it to finish the hire.' : undefined}
>
  <StatusDot status={HEALTH_TONE[health]} pulse={health === 'warming'} />
  {HEALTH_LABEL[health]}
</span>
