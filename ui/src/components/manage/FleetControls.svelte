<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Activity, Play, Repeat } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import {
    reconcileFleet,
    rollFleet,
    useFleetRoll,
    type FleetRollStart,
    type ReconcileResult,
  } from '@/lib/fleet-defs'
  import { slide } from '@/lib/motion'

  // THE CONTROLS an admin reaches for across the whole instance, each one
  // the same call its own page makes: roll every agent (Agents), start the
  // ones that are stopped (the fleet reconcile), and re-probe every MCP
  // server now rather than at the next hourly pass (the MCP health job).
  // All three are admin-only on the server, so the overview mounts this
  // panel for admins only.
  //
  // Each says what it is about to do on the button, and answers in a line
  // under its row — the deploying-action rule from UI-CONVENTIONS.
  const queryClient = useQueryClient()

  // ── Roll all ──
  const rollQuery = useFleetRoll(() => true)
  const anyRolling = $derived(rollQuery.data?.rolling ?? false)
  let rollConfirming = $state(false)
  let rollStopped = $state(false)
  let rollBusy = $state(false)
  let rollStart = $state<FleetRollStart | null>(null)
  const startRoll = async () => {
    rollBusy = true
    try {
      rollStart = await rollFleet(rollStopped)
      rollConfirming = false
      await queryClient.invalidateQueries({ queryKey: ['fleet-roll'] })
    } finally {
      rollBusy = false
    }
  }
  const batch = $derived.by(() => {
    const id = rollStart?.batch
    return id ? (rollQuery.data?.rolls ?? []).filter((r) => r.batch === id) : []
  })
  const batchRolling = $derived(batch.filter((r) => r.state === 'rolling').length)
  const batchDone = $derived(batch.filter((r) => r.state === 'done').length)
  const batchFailed = $derived(batch.filter((r) => r.state === 'failed'))
  const rollLine = $derived.by(() => {
    if (!rollStart) return null
    if (rollStart.error) return { tone: 'danger', text: `Could not start the restart: ${rollStart.error}` }
    if (batchRolling > 0) return { tone: 'muted', text: `Restarting: ${batchDone + batchFailed.length} of ${batch.length} done` }
    if ((rollStart.rolling?.length ?? 0) === 0) return { tone: 'muted', text: 'Nothing to restart.' }
    const failed = batchFailed.length > 0 ? `, ${batchFailed.length} failed: ${batchFailed.map((f) => f.displayName).join(', ')}` : ''
    return { tone: batchFailed.length > 0 ? 'danger' : 'ok', text: `Restarted ${batchDone} of ${batch.length || rollStart.rolling?.length || 0}${failed}` }
  })

  // ── Start stopped agents ──
  let startBusy = $state(false)
  let startResult = $state<ReconcileResult | null>(null)
  const startStopped = async () => {
    startBusy = true
    try {
      startResult = await reconcileFleet()
      await queryClient.invalidateQueries({ queryKey: ['fleet-containers'] })
    } finally {
      startBusy = false
    }
  }
  const startLine = $derived.by(() => {
    if (!startResult) return null
    if (startResult.error) return { tone: 'danger', text: `Could not start agents: ${startResult.error}` }
    if (startResult.pending) return { tone: 'ok', text: 'Still starting. A first start downloads the agent image, and Agents shows each one as it comes up.' }
    const started = startResult.started ?? []
    const warn = startResult.warnings?.length ? ` ${startResult.warnings.join(' ')}` : ''
    return {
      tone: startResult.warnings?.length ? 'danger' : 'ok',
      text: started.length > 0 ? `Started ${started.join(', ')}.${warn}` : `Every agent was already running.${warn}`,
    }
  })

  // ── Check MCP servers ──
  interface HealthReport {
    checked: number
    down: number
    recovered: number
    changed: number
    servers: { name: string; state: string; detail: string }[]
  }
  let mcpBusy = $state(false)
  let mcpResult = $state<{ report?: HealthReport; error?: string } | null>(null)
  const checkMcp = async () => {
    mcpBusy = true
    try {
      mcpResult = { report: await postJson<HealthReport>('/api/mcp/health') }
      await queryClient.invalidateQueries({ queryKey: ['mcp-servers'] })
      await queryClient.invalidateQueries({ queryKey: ['alerts'] })
    } catch (e) {
      mcpResult = { error: errorMessage(e) }
    } finally {
      mcpBusy = false
    }
  }
  const mcpLine = $derived.by(() => {
    if (!mcpResult) return null
    if (mcpResult.error || !mcpResult.report) return { tone: 'danger', text: `Could not run the check: ${mcpResult.error}` }
    const r = mcpResult.report
    if (r.checked === 0) return { tone: 'muted', text: 'No external servers to check.' }
    const down = r.servers.filter((s) => ['unreachable', 'error', 'auth-required', 'reconnect'].includes(s.state))
    const parts = [
      `Checked ${r.checked}`,
      down.length > 0 ? `${down.length} not working: ${down.map((s) => s.name).join(', ')}` : 'all answering',
      r.recovered > 0 ? `${r.recovered} recovered` : null,
      r.changed > 0 ? `${r.changed} changed their tools` : null,
    ].filter(Boolean)
    return { tone: down.length > 0 ? 'danger' : 'ok', text: `${parts.join(' · ')}.` }
  })

  const TONE: Record<string, string> = { danger: 'text-danger', ok: 'text-fg', muted: 'text-muted' }
  // One width for the three, so the column of buttons reads as one control.
  const ACTION = 'w-[8.5rem]'
</script>

{#snippet result(line: { tone: string; text: string } | null, resting: string)}
  {#if line}<span class={TONE[line.tone]}>{line.text}</span>{:else}{resting}{/if}
{/snippet}

<SettingsSection title="Fleet controls">
  <SettingsGroup>
    <div>
      <SettingsRow label="Restart all agents">
        {#snippet hint()}{@render result(rollLine, 'Each agent is offline for about a minute while it restarts.')}{/snippet}
        <Button
          variant="outline"
          size="sm"
          class={ACTION}
          disabled={anyRolling || rollConfirming || rollBusy}
          onclick={() => (rollConfirming = true)}
          title={anyRolling ? 'A restart is already in progress' : undefined}
        >
          {#if anyRolling}<WaitingMark site="manage/fleet-roll" size={12} />{:else}<Repeat size={14} />{/if}
          {anyRolling ? 'Restarting' : 'Restart all'}
        </Button>
      </SettingsRow>
      {#if rollConfirming}
        <div transition:slide={{ duration: 150 }}>
          <div class="flex flex-wrap items-center justify-between gap-3 border-t border-line-subtle px-5 py-3">
            <Checkbox checked={rollStopped} onChange={(v) => (rollStopped = v)} label="Also start agents that are stopped" />
            <div class="flex items-center gap-2">
              <Button variant="ghost" size="sm" onclick={() => (rollConfirming = false)} disabled={rollBusy}>Cancel</Button>
              <Button size="sm" onclick={() => void startRoll()} disabled={rollBusy}>
                {#if rollBusy}<WaitingMark site="manage/fleet-roll" size={12} />{/if}
                Restart all agents
              </Button>
            </div>
          </div>
        </div>
      {/if}
    </div>
    <SettingsRow label="Start stopped agents">
      {#snippet hint()}{@render result(startLine, 'Starts every enabled agent that is not running.')}{/snippet}
      <Button variant="outline" size="sm" class={ACTION} disabled={startBusy} onclick={() => void startStopped()}>
        {#if startBusy}<WaitingMark site="manage/fleet-start" size={12} />{:else}<Play size={14} />{/if}
        Start agents
      </Button>
    </SettingsRow>
    <SettingsRow label="Check MCP servers">
      {#snippet hint()}{@render result(mcpLine, 'Tests every external server now instead of at the hourly check.')}{/snippet}
      <Button variant="outline" size="sm" class={ACTION} disabled={mcpBusy} onclick={() => void checkMcp()}>
        {#if mcpBusy}<WaitingMark site="manage/mcp-check" size={12} />{:else}<Activity size={14} />{/if}
        Check now
      </Button>
    </SettingsRow>
  </SettingsGroup>
</SettingsSection>
