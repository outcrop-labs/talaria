<script lang="ts">
  import { Archive, Copy, Play, Repeat, RotateCw, SlidersHorizontal, Square, Trash2, UserPlus } from '@lucide/svelte'
  import GeneratingBars from '@/components/ui/GeneratingBars.svelte'
  import { useSession } from '@/lib/session'
  import type { AgentDef } from '@/lib/fleet-defs'
  import AgentIconBtn from './AgentIconBtn.svelte'
  import AgentRetireModal from './AgentRetireModal.svelte'
  import { deleteForeverConfirm, RESTART_CONFIRM, useAgentControls } from './agents.svelte'

  /** The control-icon cluster (start/stop/restart/roll · manage · retire · re-hire). */
  let {
    def: d,
    running,
    onManage,
    onDuplicate,
  }: { def: AgentDef; running: boolean; onManage: () => void; onDuplicate: () => void } = $props()

  const session = useSession()
  const isAdmin = $derived(session.data?.role === 'admin')
  const controls = useAgentControls(() => d)
  let retiring = $state(false)
</script>

{#if controls.pending}
  <GeneratingBars bars={3} variant="weave" step={0.15} class="text-muted" />
{:else if !d.enabled}
  <!-- Retired agents: re-hire (re-enable + start), duplicate as a template, or
       delete forever (admin) — the only truly destructive lifecycle action. -->
  <div class="flex items-center">
    <AgentIconBtn title="Duplicate to a new agent" onClick={onDuplicate}><Copy size={15} /></AgentIconBtn>
    <AgentIconBtn title="Re-hire" onClick={() => void controls.act('unretire', 're-hiring')}><UserPlus size={15} /></AgentIconBtn>
    {#if isAdmin}
      <AgentIconBtn
        title="Delete forever"
        danger
        onClick={() => void controls.act('delete', 'Delete forever', deleteForeverConfirm(d))}
      ><Trash2 size={15} /></AgentIconBtn>
    {/if}
  </div>
{:else}
  <div class="flex items-center">
    <AgentIconBtn title="Duplicate to a new agent" onClick={onDuplicate}><Copy size={15} /></AgentIconBtn>
    <AgentIconBtn title="Manage" onClick={onManage}><SlidersHorizontal size={15} /></AgentIconBtn>
    <AgentIconBtn title="Retire" danger onClick={() => (retiring = true)}><Archive size={15} /></AgentIconBtn>
    {#if retiring}
      <AgentRetireModal def={d} onClose={() => (retiring = false)} onConfirm={() => void controls.act('retire', 'retiring')} />
    {/if}
    <!-- Start/stop stands apart from the rest — it's the lifecycle switch,
         not another management action. Filled glyphs so they read at 14px. -->
    <span aria-hidden="true" class="mx-1.5 h-4 w-px bg-line"></span>
    {#if running}
      <AgentIconBtn title="Stop" onClick={() => void controls.act('stop', 'stopping')}><Square size={14} fill="currentColor" /></AgentIconBtn>
      <AgentIconBtn
        title="Restart (quick bounce — drops any in-flight reply)"
        onClick={() => void controls.act('restart', 'restarting', RESTART_CONFIRM)}
      ><RotateCw size={14} /></AgentIconBtn>
      {#if isAdmin}
        <AgentIconBtn
          title="Roll (zero-downtime replacement — fresh container, old one finishes its replies)"
          onClick={() => void controls.act('roll', 'rolling')}
        ><Repeat size={14} /></AgentIconBtn>
      {/if}
    {:else}
      <AgentIconBtn title="Start" onClick={() => void controls.act('up', 'starting')}><Play size={14} fill="currentColor" /></AgentIconBtn>
    {/if}
  </div>
{/if}
