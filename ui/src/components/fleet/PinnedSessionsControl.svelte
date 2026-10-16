<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { patchAgentMeta, type AgentDef } from '@/lib/fleet-defs'
  import { toastError } from '@/lib/toast.svelte'

  // One Hermes session per conversation. Off, every turn re-sends the whole
  // transcript and Hermes files it under whatever session this turn's system
  // prompt hashes to — which moves between turns, so one conversation becomes
  // several Hermes sessions and its memory tools see fragments. On, the
  // conversation IS the session and Hermes keeps the history. Talaria-side
  // only: the next turn takes the other door, no restart.
  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

  const qc = useQueryClient()
  let saving = $state(false)
  const set = async (on: boolean) => {
    saving = true
    const res = await patchAgentMeta(def.id, { pinnedSessions: on })
    saving = false
    if (res.error) {
      toastError(on ? 'Could not pin this agent\'s sessions' : 'Could not unpin this agent\'s sessions', res.error)
      return
    }
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

<div>
  <div class="mb-1 flex items-center gap-1.5">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Pinned sessions</span>
    <InfoTip text="Keeps each conversation as one session inside the agent, so its own memory — compaction, session search — covers the whole thread instead of pieces of it. Applies to conversations started after it is on; older ones keep working the way they did." />
  </div>
  {#if canManage}
    <Toggle
      checked={def.pinnedSessions}
      disabled={saving}
      onChange={(on) => void set(on)}
      label={def.pinnedSessions ? 'On · Hermes-native memory' : 'Off'}
    />
  {:else}
    <div class="text-fg">{def.pinnedSessions ? 'On · Hermes-native memory' : 'Off'}</div>
  {/if}
</div>
