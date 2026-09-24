<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { patchAgentMeta, type AgentDef } from '@/lib/fleet-defs'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import WorkbenchRepos from './WorkbenchRepos.svelte'

  // The one developer setting. On sets the agent up end to end (the dev
  // sandbox, Oh My Pi as its coding harness, and the Workbench tools
  // start_job / finish_job) and rolls the agent so it lands. Which repos
  // it may touch stays an explicit pick below; the switch never grants those.
  let { def, isAdmin }: { def: AgentDef; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  let saving = $state(false)
  const set = async (on: boolean) => {
    saving = true
    const res = await patchAgentMeta(def.id, { developer: on })
    saving = false
    if (res.error) {
      toastError(on ? 'Could not make this a Developer Agent' : 'Could not turn off Developer Agent', res.error)
      return
    }
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

<div>
  <div class="mb-1 flex items-center gap-1.5">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Developer Agent</span>
    <InfoTip text="Lets this agent do coding work from tickets: a sandbox, Oh My Pi as its coding harness, and the Workbench tools that open branches and PRs. Turning it on or off restarts the agent so the change takes effect. It can only touch the repos you pick below." />
  </div>
  {#if isAdmin}
    <Toggle
      checked={def.developer}
      disabled={saving}
      onChange={(on) => void set(on)}
      label={def.developer ? 'On · Oh My Pi' : 'Off'}
    />
  {:else}
    <div class="text-fg">{def.developer ? 'On · Oh My Pi' : 'Off'}</div>
  {/if}
  {#if isAdmin && def.developer}
    <div transition:slide={{ duration: 150 }}>
      <WorkbenchRepos agentId={def.id} />
    </div>
  {/if}
</div>
