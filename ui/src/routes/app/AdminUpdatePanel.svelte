<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
  import { relativeTime } from '@/lib/fleet'

  // In-app updates, the rolling kind: an update pulls the new image, brings
  // a second container up, waits for it to be healthy, and only then moves
  // traffic and retires the old one — the panel follows along by polling,
  // and the failure window is the drain, not the deploy. Manual by default;
  // the auto toggle opts into the scheduled check, which is the only path
  // that updates without a person.
  interface Pin {
    digest: string
    version: string
  }
  interface RunRecord {
    state: 'pulling' | 'starting' | 'cutting-over' | 'done' | 'failed' | 'rolled-back'
    from: Pin
    to: Pin
    by: 'manual' | 'auto'
    startedAt: string
    finishedAt: string | null
    error: string | null
  }
  interface UpdateStatus {
    mode: 'image' | 'checkout' | 'dev' | 'off'
    sentence: string | null
    migrated: boolean
    running: { version: string | null; digest: string | null; slot: 'a' | 'b' | null; project: string }
    autoUpdate: boolean
    machineKeySet: boolean
    available: Pin | null
    lastCheck: { at: string; available: Pin | null; error: string | null } | null
    lastRun: RunRecord | null
    history: RunRecord[]
  }

  const qc = useQueryClient()
  // `kicked` is local: it remembers that THIS browser pressed Update, so the
  // polling keeps going through the cutover itself — the edge keeps serving,
  // but the drain is a real (brief) window of failed requests, and the query
  // has nothing fresh to say through it.
  let kicked = $state(false)
  const query = createQuery(() => ({
    queryKey: ['admin-updates'],
    queryFn: (): Promise<UpdateStatus> => getJson('/api/admin/updates'),
    retry: false,
  }))
  const status = $derived(query.data)
  const IN_FLIGHT: RunRecord['state'][] = ['pulling', 'starting', 'cutting-over']
  const running = $derived(status?.lastRun ? IN_FLIGHT.includes(status.lastRun.state) : false)

  // Poll on an effect rather than refetchInterval: the interval has to ride
  // through FAILED fetches (the old container is draining; every request
  // errors) and read reactive state the options object cannot reach without
  // self-referencing the query it is creating.
  $effect(() => {
    if (!(kicked || running)) return
    const timer = setInterval(() => void query.refetch(), 3_000)
    return () => clearInterval(timer)
  })

  let busy = $state<'check' | 'apply' | 'rollback' | 'mint' | null>(null)
  let error = $state<string | null>(null)
  let mintedKey = $state<string | null>(null)

  const check = async () => {
    busy = 'check'
    error = null
    try {
      await postJson<{ available: Pin }>('/api/admin/updates', { action: 'check' })
    } catch (e) {
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  const apply = async () => {
    const ok = await confirm({
      title: 'Update now?',
      message:
        'Talaria pulls the new image, brings it up beside this one, and moves traffic once it is healthy. Nobody is interrupted; this page follows along on its own.',
      confirmLabel: 'Update now',
    })
    if (!ok) return
    busy = 'apply'
    error = null
    kicked = true
    try {
      await postJson<{ started: true; to: Pin }>('/api/admin/updates', { action: 'apply' })
    } catch (e) {
      kicked = false
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  const rollback = async () => {
    const ok = await confirm({
      title: 'Roll back?',
      message:
        'The previous container comes back up, takes the traffic, and this version stops. Rolling forward again is a normal update.',
      confirmLabel: 'Roll back',
    })
    if (!ok) return
    busy = 'rollback'
    error = null
    kicked = true
    try {
      await postJson<{ started: true }>('/api/admin/updates', { action: 'rollback' })
    } catch (e) {
      kicked = false
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  const setAuto = async (on: boolean) => {
    error = null
    try {
      await putJson<{ autoUpdate: boolean }>('/api/admin/updates', { autoUpdate: on })
    } catch (e) {
      error = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  const mint = async () => {
    const ok = await confirm({
      title: 'Generate a deploy key?',
      message:
        'The key authorizes the deploy script (check, apply, roll back) without a session. It is shown once and stored as a hash — copy it now or mint a new one later.',
      confirmLabel: 'Generate',
    })
    if (!ok) return
    busy = 'mint'
    error = null
    try {
      const res = await postJson<{ key: string }>('/api/admin/updates', { action: 'mint-key' })
      mintedKey = res.key
    } catch (e) {
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  // The update landed once the serving container says so; only then may the
  // cutover polling stop.
  $effect(() => {
    if (kicked && status && !running) kicked = false
  })

  const upToDate = $derived(
    !!status && !!status.available && !!status.running.digest
      ? status.available.digest === status.running.digest
      : false,
  )
  const canRollBack = $derived(status?.migrated === true && status.lastRun?.state === 'done')
</script>

<Panel>
  <SectionHeader
    title="Updates"
    info="Keep this Talaria current from right here. An update rolls: the new container comes up beside the live one, takes traffic once healthy, and the old one drains. No server shell needed."
  />

  {#if query.isPending}
    <Skeleton class="h-4 w-64" />
  {:else if status === undefined}
    <QueryError variant="inline" error={query.error} title="Could not load update status" onRetry={() => void query.refetch()} />
  {:else}
    <div class="space-y-3">
      {#if status.mode !== 'image'}
        <!-- The engine's own refusal sentence IS the panel text — checkout,
             dev, and off installs get the truth about why there are no
             buttons here, straight from the api that would refuse them. -->
        <p class="text-sm text-muted">{status.sentence}</p>
      {:else if running}
        <div class="flex items-center gap-2 text-sm text-fg">
          <span class="h-2 w-2 shrink-0 animate-pulse rounded-full bg-[var(--theme-accent)]"></span>
          Rolling out {status.lastRun?.to.version}. Traffic moves once the new container is healthy.
        </div>
      {:else if kicked}
        <!-- Requests are failing against a drain in flight: the honest line
             is the cutover, not an error card. -->
        <div class="flex items-center gap-2 text-sm text-fg">
          <span class="h-2 w-2 shrink-0 animate-pulse rounded-full bg-[var(--theme-accent)]"></span>
          Moving traffic to the new container. Hang tight.
        </div>
      {:else}
        <p class="text-sm text-muted">
          Running
          <span class="font-mono text-[13px] text-fg">{status.running.version ?? 'a local build'}</span>
          {#if status.running.slot}<span class="whitespace-nowrap">(slot {status.running.slot})</span>{/if}
        </p>
        {#if !status.migrated}
          <!-- Every install starts here: the engine ships dormant, and
               deploys keep flowing through the orchestrator exactly as they
               always have until an admin adopts the update engine. -->
          <p class="text-sm text-muted">
            This install deploys the way it always has — the in-app rolling updater takes over only after an admin adopts it.
          </p>
          <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void check()}>
            {busy === 'check' ? 'Checking...' : 'Check for updates'}
          </Button>
        {:else if upToDate}
          <div class="flex items-center gap-2">
            <p class="text-sm text-muted">You're up to date.</p>
            <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void check()}>
              {busy === 'check' ? 'Checking...' : 'Check again'}
            </Button>
          </div>
        {:else if status.available}
          <p class="text-sm text-fg">
            <span class="font-mono text-[13px]">{status.available.version}</span> is available.
          </p>
          <div class="flex items-center gap-2">
            <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void apply()}>
              {busy === 'apply' ? 'Starting...' : 'Update now'}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void check()}>
              {busy === 'check' ? 'Checking...' : 'Check again'}
            </Button>
          </div>
        {:else}
          <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void check()}>
            {busy === 'check' ? 'Checking...' : 'Check for updates'}
          </Button>
        {/if}
      {/if}

      {#if status.lastCheck?.error && !running}
        <p class="text-sm text-danger">{status.lastCheck.error}</p>
      {/if}

      {#if status.mode === 'image' && status.migrated && !running}
        <div class="border-t border-line pt-3">
          <Checkbox checked={status.autoUpdate} onChange={(v) => void setAuto(v)} label="Update automatically" />
          <p class="mt-1 text-xs text-muted">
            Checks every few hours and rolls updates out on its own. Off by default.
          </p>
        </div>
        {#if canRollBack}
          <div class="border-t border-line pt-3">
            <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void rollback()}>
              {busy === 'rollback' ? 'Rolling back...' : `Roll back to ${status.lastRun?.from.version}`}
            </Button>
          </div>
        {/if}
      {/if}

      {#if status.mode === 'image'}
        <div class="border-t border-line pt-3">
          {#if mintedKey}
            <p class="text-xs text-muted">Deploy key (shown once — copy it now):</p>
            <div class="mt-1 flex items-center gap-2">
              <code class="break-all rounded bg-muted px-2 py-1 font-mono text-xs">{mintedKey}</code>
              <CopyButton value={mintedKey} title="Copy key" />
            </div>
          {:else}
            <div class="flex items-center gap-2">
              <Button size="sm" variant="ghost" disabled={busy !== null} onclick={() => void mint()}>
                {busy === 'mint' ? 'Generating...' : status.machineKeySet ? 'Regenerate deploy key' : 'Generate deploy key'}
              </Button>
            </div>
            <p class="mt-1 text-xs text-muted">
              For the deploy script (header <code class="font-mono">x-talaria-key</code>): check, apply, roll back — no session. Stored as a hash.
            </p>
          {/if}
        </div>
      {/if}

      {#if status.lastRun?.state === 'failed' && status.lastRun.error}
        <p class="text-sm text-danger">{status.lastRun.error}</p>
      {:else if status.lastRun?.finishedAt}
        <p class="text-xs text-muted">
          {status.lastRun.state === 'rolled-back' ? 'Rolled back' : 'Last updated'} {relativeTime(status.lastRun.finishedAt)}.
        </p>
      {/if}

      {#if error}
        <p class="text-sm text-danger">{error}</p>
      {/if}
    </div>
  {/if}
</Panel>
