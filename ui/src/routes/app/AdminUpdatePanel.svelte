<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import RowStatus from '@/components/admin/RowStatus.svelte'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
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
    adoption: { stage: 'edge-pending' | 'edge-ready' | 'cutover'; edgePort: string } | null
    autoUpdate: boolean
    machineKeySet: boolean
    available: Pin | null
    lastCheck: { at: string; available: Pin | null; error: string | null } | null
    lastRun: RunRecord | null
    history: RunRecord[]
  }

  const qc = useQueryClient()
  const autosave = useAutosave()
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

  let busy = $state<'check' | 'apply' | 'rollback' | 'adopt' | 'mint' | null>(null)
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

  // The one-time handover: this install becomes its own deployer. The panel
  // always takes the INHERIT path (no edgePort) — the fresh-port path is the
  // migration script's, for proxies that can repoint without a cut.
  const adopt = async () => {
    const ok = await confirm({
      title: 'Take over updates?',
      message:
        'Talaria starts installing its own updates. The first handover interrupts traffic once, for a few seconds. Every update after that installs with no interruption.',
      confirmLabel: 'Take over',
    })
    if (!ok) return
    busy = 'adopt'
    error = null
    kicked = true
    try {
      await postJson<{ started: boolean }>('/api/admin/updates', { action: 'adopt' })
    } catch (e) {
      kicked = false
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  // The fresh-port handover's second call, from the panel: heals a stuck
  // hold (the migration script died between repointing the proxy and
  // finishing). Arriving here at all means this page loaded through the
  // edge — which is exactly the proof the finish waits for.
  const finishAdoption = async () => {
    const ok = await confirm({
      title: 'Finish the handover?',
      message:
        'The old version stops and the handover is complete. Traffic already arrives through the new front door, which is how this page loaded.',
      confirmLabel: 'Finish',
    })
    if (!ok) return
    busy = 'adopt'
    error = null
    kicked = true
    try {
      await postJson<{ stage: string }>('/api/admin/updates', { action: 'adopt' })
    } catch (e) {
      kicked = false
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  // The fresh-port handover's heal call when the edge never came up: the
  // same adopt POST (on blue it replays the edge's compose and waits for
  // health — it cannot cut over, the proxy has not repointed), which is
  // why it needs no confirm. `kicked` stays false: this call never drains
  // the container the page is served from.
  const retryEdge = async () => {
    busy = 'adopt'
    error = null
    try {
      await postJson<{ stage: string }>('/api/admin/updates', { action: 'adopt' })
    } catch (e) {
      error = errorMessage(e)
    }
    busy = null
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  const setAuto = async (on: boolean) => {
    error = null
    try {
      await putJson<{ autoUpdate: boolean }>('/api/admin/updates', { autoUpdate: on })
      autosave.flash()
    } catch (e) {
      error = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['admin-updates'] })
  }

  const mint = async () => {
    const ok = await confirm({
      title: 'Generate a deploy key?',
      message:
        'The key lets a deploy script check, update and roll back without signing in. It is shown once and only a hash is stored, so copy it now or generate a new one later.',
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

  const checkItem = (label: string): ContextMenuEntry => ({ label, disabled: busy !== null, onSelect: () => void check() })
  const rollbackItems = $derived.by((): ContextMenuEntry[] =>
    canRollBack
      ? [{ label: `Roll back to ${status?.lastRun?.from.version}`, disabled: busy !== null, onSelect: () => void rollback() }]
      : [],
  )
  const RUN_WORD: Record<RunRecord['state'], string> = {
    pulling: 'Downloading',
    starting: 'Starting',
    'cutting-over': 'Switching over',
    done: 'Updated',
    failed: 'Failed',
    'rolled-back': 'Rolled back',
  }
</script>

{#snippet working(text: string, detail: string)}
  <SettingsRow label={text} hint={detail}>
    {#snippet lead()}<span class="h-2 w-2 shrink-0 animate-pulse rounded-full bg-[var(--theme-accent)]"></span>{/snippet}
  </SettingsRow>
{/snippet}

<!-- Declared out here because the rows below sit where `status` is the update
     state, and a `status` snippet inside them would shadow it. -->
{#snippet versionStatus()}
  {#if busy === 'check'}
    <RowStatus status="idle">Checking</RowStatus>
  {:else if status?.migrated && status.available && !upToDate}
    <RowStatus status="warn">Update available</RowStatus>
  {:else if status?.migrated && upToDate}
    <RowStatus status="ok">Up to date</RowStatus>
  {:else if status?.migrated}
    <RowStatus status="idle">Not checked</RowStatus>
  {/if}
{/snippet}
{#snippet deployKeyStatus()}
  {#if busy === 'mint'}
    <RowStatus status="idle">Generating</RowStatus>
  {:else}
    <RowStatus status={status?.machineKeySet ? 'ok' : 'idle'}>{status?.machineKeySet ? 'Set' : 'Not set'}</RowStatus>
  {/if}
{/snippet}

<SettingsSection
  title="Updates"
  description="Keep this Talaria current without touching the server."
  info="An update starts the new version beside the running one, moves traffic to it once it is healthy, and then stops the old one, so nobody is interrupted."
>
  <div class="space-y-3">
    <SettingsGroup>
      <QueryState query={query} errorTitle="Could not load update status" errorVariant="compact">
        {#snippet skeleton()}
          <SettingsRow label="Version"><Skeleton class="h-8 w-28" /></SettingsRow>
        {/snippet}
        {#snippet children(status)}
          {#if status.mode !== 'image'}
            <!-- The engine's own refusal sentence IS the row text — checkout,
                 dev, and off installs get the truth about why there are no
                 buttons here, straight from the api that would refuse them. -->
            <SettingsRow label="Updates are not managed here" hint={status.sentence ?? undefined} />
          {:else if running}
            {@render working(`Updating to ${status.lastRun?.to.version ?? 'the new version'}`, 'Traffic moves once the new version is healthy.')}
            {#if status.adoption?.stage === 'edge-pending'}
              <SettingsRow
                label="The front door has not started"
                hint={`Nothing is answering on port ${status.adoption.edgePort} yet. If retrying does not help, check its container on the server.`}
              >
                <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void retryEdge()}>
                  {busy === 'adopt' ? 'Retrying' : 'Retry'}
                </Button>
              </SettingsRow>
            {:else if status.adoption?.stage === 'edge-ready'}
              <SettingsRow
                label="Ready to hand over"
                hint={`The front door is up on port ${status.adoption.edgePort}. Traffic moves when your proxy points at it.`}
              >
                <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void finishAdoption()}>
                  {busy === 'adopt' ? 'Finishing' : 'Finish the handover'}
                </Button>
              </SettingsRow>
            {/if}
          {:else if kicked}
            <!-- Requests are failing against a drain in flight: the honest line
                 is the cutover, not an error card. -->
            {@render working('Moving traffic to the new version', 'This page follows along on its own.')}
          {:else}
            <SettingsRow label="Version" status={versionStatus}>
              {#snippet meta()}
                <code class="shrink-0 font-mono text-xs text-muted">{status.running.version ?? 'local build'}</code>
              {/snippet}
              {#snippet hint()}
                {#if !status.migrated}
                  <!-- Every install starts here: the engine ships dormant, and
                       deploys keep flowing through the orchestrator exactly as
                       they always have until an admin adopts the update engine. -->
                  This install is still updated from the server. Take over to update it from here.
                {:else if upToDate}
                  Nothing newer is out.
                {:else if status.available}
                  Version <span class="font-mono text-xs text-fg">{status.available.version}</span> is ready to install.
                {:else}
                  Check for updates from the menu to see whether a newer version is out.
                {/if}
              {/snippet}
              {#if !status.migrated}
                <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void adopt()}>
                  {busy === 'adopt' ? 'Taking over' : 'Take over updates'}
                </Button>
                <SettingsRowMenu label="More update actions" items={[checkItem('Check for updates')]} />
              {:else if status.available && !upToDate}
                <Button size="sm" variant="outline" disabled={busy !== null} onclick={() => void apply()}>
                  {busy === 'apply' ? 'Starting' : 'Update now'}
                </Button>
                <SettingsRowMenu label="More update actions" items={[checkItem('Check again'), ...rollbackItems]} />
              {:else}
                <!-- Nothing to install, so checking is a menu item and the
                     status says where things stand. -->
                <SettingsRowMenu label="More update actions" items={[checkItem(upToDate ? 'Check again' : 'Check for updates'), ...rollbackItems]} />
              {/if}
            </SettingsRow>
          {/if}

          {#if status.mode === 'image' && status.migrated && !running}
            <SettingsRow label="Update automatically" hint="Checks every few hours and installs updates on its own." saved={autosave.saved()}>
              <Toggle title="Update automatically" checked={status.autoUpdate} onChange={(v) => void setAuto(v)} />
            </SettingsRow>
          {/if}

          {#if status.mode === 'image'}
            <SettingsRow
              label="Deploy key"
              hint="Lets a deploy script check, update and roll back without signing in."
              info="The script sends it in the x-talaria-key header. Only a hash is stored, so the key is shown once. Generate a new one if you lose it."
              status={deployKeyStatus}
            >
              <!-- A deploy key is optional, so making one is a menu item
                   whether or not one is set. -->
              <SettingsRowMenu
                label="Actions for the deploy key"
                items={[{ label: status.machineKeySet ? 'Regenerate key' : 'Generate key', disabled: busy !== null, onSelect: () => void mint() }]}
              />
            </SettingsRow>
            {#if mintedKey}
              <SettingsRow label="Copy your deploy key now" hint="It will not be shown again." stacked>
                <code class="min-w-0 flex-1 break-all rounded-md border border-line bg-surface px-3 py-2 font-mono text-xs text-fg">{mintedKey}</code>
                <CopyButton value={mintedKey} label="Copy" title="Copy key" />
              </SettingsRow>
            {/if}
          {/if}

          {#if status.lastCheck?.error && !running}
            <p class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{status.lastCheck.error}</p>
          {/if}
          {#if status.lastRun?.state === 'failed' && status.lastRun.error}
            <p class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{status.lastRun.error}</p>
          {/if}
        {/snippet}
      </QueryState>
      {#if error}
        <p class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{error}</p>
      {/if}
    </SettingsGroup>

    {#if status?.history.length}
      <Disclosure title="Update history" variant="card">
        <ul class="space-y-2 font-sans text-[13px] leading-5">
          {#each status.history as run (run.startedAt)}
            <li class="grid grid-cols-[minmax(0,1fr)_8rem_6rem_5rem] items-center gap-4">
              <span class="truncate font-mono text-xs text-fg">{run.from.version} → {run.to.version}</span>
              <span class={run.state === 'failed' ? 'text-danger' : 'text-muted'} title={run.error ?? undefined}>{RUN_WORD[run.state]}</span>
              <span class="text-muted">{run.by === 'auto' ? 'Automatic' : 'By an admin'}</span>
              <span class="text-right tabular-nums text-muted" title={new Date(run.startedAt).toLocaleString()}>
                {relativeTime(run.finishedAt ?? run.startedAt)}
              </span>
            </li>
          {/each}
        </ul>
      </Disclosure>
    {/if}
  </div>
</SettingsSection>
