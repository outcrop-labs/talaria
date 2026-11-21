<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { patchAgentMeta, type AgentDef } from '@/lib/fleet-defs'
  import { githubPermissionsKey } from '@/lib/github-access'
  import { toastError } from '@/lib/toast.svelte'

  // The one developer setting, as a row: on sets the agent up end to end
  // (the dev sandbox, Oh My Pi as its coding harness, and the Workbench tools
  // start_job / finish_job) and rolls the agent so it lands. Its managers and
  // admins switch it (PATCH /api/fleet/defs/{id}); everyone else reads On or
  // Off. Which repos it may touch, and what its token carries, stay explicit
  // picks: the switch never grants either. The agent's Overview and its
  // GitHub access both draw this row, so the two never say different things.
  let {
    def,
    canManage,
    context = 'summary',
  }: {
    def: AgentDef
    canManage: boolean
    /** `editor`: drawn above the repositories and permissions it decides
     *  about, so its hint speaks of them. */
    context?: 'summary' | 'editor'
  } = $props()

  const qc = useQueryClient()
  const autosave = useAutosave()
  let saving = $state(false)
  const set = async (on: boolean) => {
    saving = true
    const res = await patchAgentMeta(def.id, { developer: on })
    saving = false
    if (res.error) {
      toastError(on ? 'Could not make this a Developer Agent' : 'Could not turn off Developer Agent', res.error)
      return
    }
    autosave.flash()
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
    await qc.invalidateQueries({ queryKey: githubPermissionsKey(def.id) })
  }

  const hint = $derived.by(() => {
    if (def.developer && !def.enabled) return 'Retired: no git credential.'
    if (context === 'editor') {
      return def.developer ? 'On: its git gets a credential for the repositories below.' : 'Off: no git credential and no push, whatever is granted below.'
    }
    return def.developer ? 'On. It does coding work from tickets with Oh My Pi.' : 'Lets it do coding work from tickets.'
  })
</script>

<SettingsRow
  label="Developer agent"
  {hint}
  info="A sandbox, Oh My Pi as its coding harness, and the Workbench tools that open branches and pull requests. Turning it on or off restarts the agent. It can only touch the repositories you pick."
  saved={autosave.saved()}
>
  {#if canManage}
    <Toggle checked={def.developer} disabled={saving} onChange={(on) => void set(on)} title="Developer agent" />
  {:else}
    <span class="font-sans text-[13px] leading-5 text-fg">{def.developer ? 'On' : 'Off'}</span>
  {/if}
</SettingsRow>
