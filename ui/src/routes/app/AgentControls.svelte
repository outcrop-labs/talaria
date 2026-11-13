<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import type { AgentFrame } from '@/components/fleet/AgentItem.svelte'
  import type { AgentContainers, AgentDef, FleetRollRow } from '@/lib/fleet-defs'
  import { healthOf, settleStillWorking, STILL_WORKING_LABEL, stillWorkingOf } from './agents.svelte'

  /** The one visible action of a tile or row: Start, when the agent is
   *  stopped. Everything else is in the agent's menu. */
  let {
    def: d,
    containers,
    running,
    roll,
    act,
    pending,
  }: {
    def: AgentDef
    containers: AgentContainers | null
    running: boolean
    /** This agent's row in the roll picture (admins see it): an apply still
     *  going on reads its verdict there. */
    roll?: FleetRollRow
    act: AgentFrame['act']
    pending: AgentFrame['pending']
  } = $props()

  // A draft (TALA-11) was hired without a container on purpose. Its Start is
  // the same 'up' action, and finishing the hire is what it means here.
  const draft = $derived(healthOf(d, containers).health === 'draft')
  // A verb the route answered 202 for is still being carried out: noted until
  // the roster's next reading shows the state it was for (agents.svelte.ts).
  const working = $derived(stillWorkingOf(d.id))
  const workingLabel = $derived.by(() => {
    if (!working) return ''
    const label = STILL_WORKING_LABEL[working.action]
    return label.charAt(0).toUpperCase() + label.slice(1)
  })
  $effect(() => {
    settleStillWorking(d, containers, roll)
  })
</script>

{#if working}
  <span
    class="relative z-10 inline-flex shrink-0 items-center gap-1.5 font-sans text-[13px] leading-5 text-muted"
    data-testid="agent-still-working"
    title="Docker has not finished this yet, and the request stopped waiting for it. It goes on. The roster shows the agent's state when it lands."
  >
    <WaitingMark site="fleet/agent-controls" size={11} class="text-muted" />
    {workingLabel}
  </span>
{/if}
{#if pending}
  <span class="grid h-8 w-8 shrink-0 place-items-center"><WaitingMark site="fleet/agent-controls" size={12} class="text-muted" /></span>
{:else if d.enabled && d.canManage && !running}
  <Button
    variant="outline"
    size="sm"
    class="relative z-10 shrink-0"
    title={draft ? 'Start the agent and finish the hire' : undefined}
    onclick={() => void act('up', 'starting')}
  >
    Start
  </Button>
{/if}
