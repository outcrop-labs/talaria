<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Activity, Play, Repeat } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Panel from '@/components/ui/Panel.svelte'
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
    if (rollStart.error) return { tone: 'danger', text: `Could not start the roll: ${rollStart.error}` }
    if (batchRolling > 0) return { tone: 'muted', text: `Rolling: ${batchDone + batchFailed.length} of ${batch.length} done` }
    if ((rollStart.rolling?.length ?? 0) === 0) return { tone: 'muted', text: 'Nothing to roll.' }
    const failed = batchFailed.length > 0 ? `, ${batchFailed.length} failed: ${batchFailed.map((f) => f.displayName).join(', ')}` : ''
    return { tone: batchFailed.length > 0 ? 'danger' : 'ok', text: `Rolled ${batchDone} of ${batch.length || rollStart.rolling?.length || 0}${failed}` }
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
</script>

{#snippet result(line: { tone: string; text: string } | null)}
  {#if line}
    <p transition:slide={{ duration: 150 }} class="mt-1.5 font-sans text-xs {TONE[line.tone]}">{line.text}</p>
  {/if}
{/snippet}

<Panel class="px-4 py-3">
  <div class="mb-1 flex min-h-6 items-center">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Controls</span>
  </div>
  <ul class="divide-y divide-line-subtle">
    <li class="py-3">
      <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
        <div class="min-w-0 flex-1 basis-56">
          <div class="font-sans text-sm text-fg">Roll every agent</div>
          <div class="font-sans text-xs text-muted">A fresh container each; the old one finishes its replies first.</div>
        </div>
        <Button
          variant="outline"
          size="sm"
          disabled={anyRolling || rollConfirming || rollBusy}
          onclick={() => (rollConfirming = true)}
          title={anyRolling ? 'A roll is already in progress' : undefined}
        >
          {#if anyRolling}<WaitingMark site="manage/fleet-roll" size={12} />{:else}<Repeat size={14} />{/if}
          {anyRolling ? 'Rolling' : 'Roll all'}
        </Button>
      </div>
      {#if rollConfirming}
        <div transition:slide={{ duration: 150 }} class="mt-2 flex flex-wrap items-center justify-between gap-3 rounded-md border border-line-subtle px-3 py-2">
          <Checkbox checked={rollStopped} onChange={(v) => (rollStopped = v)} label="Also start and roll agents that are stopped" />
          <div class="flex items-center gap-2">
            <Button variant="ghost" size="sm" onclick={() => (rollConfirming = false)} disabled={rollBusy}>Cancel</Button>
            <Button size="sm" onclick={() => void startRoll()} disabled={rollBusy}>
              {#if rollBusy}<WaitingMark site="manage/fleet-roll" size={12} />{/if}
              Roll all agents
            </Button>
          </div>
        </div>
      {/if}
      {@render result(rollLine)}
    </li>
    <li class="py-3">
      <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
        <div class="min-w-0 flex-1 basis-56">
          <div class="font-sans text-sm text-fg">Start stopped agents</div>
          <div class="font-sans text-xs text-muted">Re-renders configs and starts every enabled agent that is not running.</div>
        </div>
        <Button variant="outline" size="sm" disabled={startBusy} onclick={() => void startStopped()}>
          {#if startBusy}<WaitingMark site="manage/fleet-start" size={12} />{:else}<Play size={14} />{/if}
          Start stopped
        </Button>
      </div>
      {@render result(startLine)}
    </li>
    <li class="py-3">
      <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
        <div class="min-w-0 flex-1 basis-56">
          <div class="font-sans text-sm text-fg">Check MCP servers now</div>
          <div class="font-sans text-xs text-muted">Re-probes every external server instead of waiting for the hourly pass.</div>
        </div>
        <Button variant="outline" size="sm" disabled={mcpBusy} onclick={() => void checkMcp()}>
          {#if mcpBusy}<WaitingMark site="manage/mcp-check" size={12} />{:else}<Activity size={14} />{/if}
          Check now
        </Button>
      </div>
      {@render result(mcpLine)}
    </li>
  </ul>
</Panel>
