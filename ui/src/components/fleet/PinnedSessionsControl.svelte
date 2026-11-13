<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
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

<SettingsRow
  label="Pinned sessions"
  hint="Each conversation stays one session, so the agent's memory covers the whole thread."
  info="Applies to conversations started after it is on. Older ones keep working the way they did."
>
  {#if canManage}
    <Toggle checked={def.pinnedSessions} disabled={saving} onChange={(on) => void set(on)} title="Pinned sessions" />
  {:else}
    <span class="font-sans text-[13px] leading-5 text-fg">{def.pinnedSessions ? 'On' : 'Off'}</span>
  {/if}
</SettingsRow>
