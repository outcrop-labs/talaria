<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { patchAgentMeta, type AgentDef } from '@/lib/fleet-defs'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import CodingAccounts from './CodingAccounts.svelte'
  import WorkbenchRepos from './WorkbenchRepos.svelte'

  // The one developer setting. On sets the agent up end to end (the dev
  // sandbox, Oh My Pi as its coding harness, and the Workbench tools
  // start_job / finish_job) and rolls the agent so it lands. Which repos
  // it may touch stays an explicit pick below; the switch never grants those.
  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

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
  }
</script>

<div>
  <SettingsRow
    label="Developer agent"
    hint={def.developer ? 'On. It does coding work from tickets with Oh My Pi.' : 'Lets it do coding work from tickets.'}
    info="A sandbox, Oh My Pi as its coding harness, and the Workbench tools that open branches and pull requests. Turning it on or off restarts the agent. It can only touch the repositories you pick."
    saved={autosave.saved()}
  >
    {#if canManage}
      <Toggle checked={def.developer} disabled={saving} onChange={(on) => void set(on)} title="Developer agent" />
    {:else}
      <span class="font-sans text-[13px] leading-5 text-fg">{def.developer ? 'On' : 'Off'}</span>
    {/if}
  </SettingsRow>
  {#if canManage && def.developer}
    <div transition:slide={{ duration: 150 }} class="space-y-6 border-t border-line-subtle px-5 py-4">
      <WorkbenchRepos agentId={def.id} />
      <!-- Coding accounts sit under the repo grants because they answer the
           next question in the same breath: this agent may touch these repos,
           and it codes on this plan. The panel renders nothing when the
           feature is off (its route 404s, which the panel treats as "absent"
           rather than an error). -->
      <CodingAccounts agentId={def.id} />
    </div>
  {/if}
</div>
