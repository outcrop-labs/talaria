<script lang="ts">
  import { Archive, Copy, Play, Repeat, RotateCw, SlidersHorizontal, Square, Trash2, UserPlus } from '@lucide/svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import type { AgentContainers, AgentDef } from '@/lib/fleet-defs'
  import IconButton from '@/components/ui/IconButton.svelte'
  import AgentRetireModal from './AgentRetireModal.svelte'
  import {
    deleteForeverConfirm,
    healthOf,
    RESTART_CONFIRM,
    settleStillWorking,
    STILL_WORKING_LABEL,
    stillWorkingOf,
    useAgentControls,
  } from './agents.svelte'

  /** The control-icon cluster (start/stop/restart/roll · manage · retire · re-hire). */
  let {
    def: d,
    containers,
    running,
    rolling = false,
    onManage,
    onDuplicate,
  }: {
    def: AgentDef
    containers: AgentContainers | null
    running: boolean
    /** Mid-roll (this agent's own, or the fleet-wide batch): the Roll control
     *  stays visible and disabled — the route would answer 409 anyway. */
    rolling?: boolean
    onManage: () => void
    onDuplicate: () => void
  } = $props()

  // Every destructive or configuring control asks the agent, not the person:
  // changing an agent belongs to its managers (docs/PERMISSIONS.md, "Agent
  // managers"), and the roster can now carry agents a viewer may see but not
  // change. Duplicate stays open — it reads this agent and writes a new one.
  const canManage = $derived(d.canManage)
  const controls = useAgentControls(() => d)
  let retiring = $state(false)
  // A draft (TALA-11): hired without a container on purpose. START is the
  // same 'up' action, labeled so it reads as the commit it is; the identity
  // stays editable through Manage; Retire/Delete stay for changing one's
  // mind. No Stop/Restart/Roll — nothing is running.
  const draft = $derived(healthOf(d, containers).health === 'draft')
  // A verb the route answered 202 for is still being carried out: noted until
  // the roster's next reading shows the state it was for (agents.svelte.ts).
  const working = $derived(stillWorkingOf(d.id))
  $effect(() => {
    settleStillWorking(d, containers)
  })
</script>

{#snippet stillWorkingNote()}
  {#if working}
    <span
      class="mr-1.5 inline-flex shrink-0 items-center gap-1 text-xs text-muted"
      data-testid="agent-still-working"
      title="Docker has not finished this yet, and the request stopped waiting for it. It goes on; the roster shows the agent's state when it lands."
    >
      <WaitingMark site="fleet/agent-controls" size={10} class="text-muted" />
      {STILL_WORKING_LABEL[working.action]}
    </span>
  {/if}
{/snippet}

{#if controls.pending}
  <WaitingMark site="fleet/agent-controls" size={12} class="text-muted" />
{:else if !d.enabled}
  <!-- Retired agents: re-hire (re-enable + start), duplicate as a template, or
       delete forever — the only truly destructive lifecycle action. -->
  <div class="flex items-center">
    {@render stillWorkingNote()}
    <IconButton title="Duplicate to a new agent" onclick={onDuplicate}><Copy size={15} /></IconButton>
    {#if canManage}
      <IconButton title="Re-hire" onclick={() => void controls.act('unretire', 're-hiring')}><UserPlus size={15} /></IconButton>
      <IconButton
        title="Delete forever"
        danger
        onclick={() => void controls.act('delete', 'Delete forever', deleteForeverConfirm(d))}
      ><Trash2 size={15} /></IconButton>
    {/if}
  </div>
{:else}
  <div class="flex items-center">
    {@render stillWorkingNote()}
    <IconButton title="Duplicate to a new agent" onclick={onDuplicate}><Copy size={15} /></IconButton>
    <!-- Manage opens for everyone who can see the agent; the modal is
         read-only unless they manage it. -->
    <IconButton title={canManage ? 'Manage' : 'View'} onclick={onManage}><SlidersHorizontal size={15} /></IconButton>
    {#if canManage}
      <IconButton title="Retire" danger onclick={() => (retiring = true)}><Archive size={15} /></IconButton>
      {#if retiring}
        <AgentRetireModal def={d} onClose={() => (retiring = false)} onConfirm={() => void controls.act('retire', 'retiring')} />
      {/if}
      <!-- Start/stop stands apart from the rest — it's the lifecycle switch,
           not another management action. Filled glyphs so they read at 14px. -->
      <span aria-hidden="true" class="mx-1.5 h-4 w-px bg-line"></span>
      {#if running}
        <IconButton title="Stop" onclick={() => void controls.act('stop', 'stopping')}><Square size={14} fill="currentColor" /></IconButton>
        <IconButton
          title="Restart (quick bounce; drops any in-flight reply)"
          onclick={() => void controls.act('restart', 'restarting', RESTART_CONFIRM)}
        ><RotateCw size={14} /></IconButton>
        <IconButton
          title={rolling ? 'Rolling now' : 'Roll (recreate in place: the container stops, a fresh one starts on the current render)'}
          disabled={rolling}
          onclick={() => void controls.act('roll', 'rolling')}
        ><Repeat size={14} /></IconButton>
      {:else if draft}
        <!-- The draft's START is the commit: the same 'up' action the roster
             has always run, with the one word that says what finishing the
             hire means here. Not renamed anywhere — the act stays 'up'. -->
        <IconButton title="Start — finish the hire" onclick={() => void controls.act('up', 'starting')}><Play size={14} fill="currentColor" /></IconButton>
      {:else}
        <IconButton title="Start" onclick={() => void controls.act('up', 'starting')}><Play size={14} fill="currentColor" /></IconButton>
      {/if}
    {/if}
  </div>
{/if}
