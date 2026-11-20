<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ExternalLink } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import type { Board } from '@/lib/boards.svelte'
  import {
    CADENCES,
    accountLabel,
    ago,
    cadenceId,
    cadenceLabel,
    cadenceMinutes,
    chipView,
    DOT_FOR_TONE,
    disconnectBoardSync,
    mappingRows,
    mappingTargets,
    lastRunNote,
    pausedHint,
    replaceSyncCredential,
    sameMap,
    statusMapOf,
    syncIntake,
    syncKey,
    syncNow,
    until,
    updateBoardSync,
    type BoardSync,
    type BoardSyncStatus,
    type CadenceId,
    type SyncOptions,
    type UpdateInput,
  } from '@/lib/board-sync'
  import { useClock } from '@/lib/clock.svelte'
  import { useBoardStatuses } from '@/lib/statuses'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import SyncMappingTable from './SyncMappingTable.svelte'
  import SyncProblems from './SyncProblems.svelte'

  // A CONNECTED BOARD's sync: where it reads from and as whom, when it last
  // ran and runs next, what it could not do (the problems, each with its fix),
  // the status mapping, what carries over, the cadence, and the credential and
  // disconnect doors. An owner changes all of it; an editor reads it and can
  // Sync now (the server refuses an editor everything else). A pause freezes
  // the sync — no schedule, no Sync now, no re-sync — so while paused the panel
  // says who paused it and when, Sync now is off, and the owner's button reads
  // Resume. The mapping, the schedule and Disconnect still work.
  let { board, sync }: { board: Board; sync: BoardSync } = $props()

  const qc = useQueryClient()
  const clock = useClock()
  const isOwner = $derived(board.role === 'owner')
  const canSync = $derived(board.role === 'owner' || board.role === 'editor')
  const view = $derived(chipView(sync, clock.now, { canResume: isOwner }))
  const paused = $derived(sync.state === 'paused')

  const statusesQuery = useBoardStatuses(() => board.id)
  const targets = $derived(mappingTargets(statusesQuery.data ?? []))
  const intake = $derived(syncIntake(targets))
  const rows = $derived(mappingRows(sync.statuses, sync.statusMap))

  // The mapping draft, seeded from the saved table — and re-seeded when the
  // saved table itself changes (another owner saved, or this one did), never
  // on an unrelated refetch while someone is mid-edit.
  let draft = $state<Record<string, string>>({})
  let seededFrom = ''
  $effect(() => {
    const saved = JSON.stringify(sync.statusMap)
    if (saved === seededFrom) return
    seededFrom = saved
    draft = { ...sync.statusMap }
  })
  const dirty = $derived(!sameMap(statusMapOf(draft, targets), statusMapOf(sync.statusMap, targets)))

  let busy = $state<string | null>(null)
  let token = $state('')

  const put = (projection: { sync: BoardSync }) =>
    qc.setQueryData<BoardSyncStatus>(syncKey(board.id), { sync: projection.sync, disconnected: null })

  async function act<T>(what: string, fn: () => Promise<T>, failed: string): Promise<T | null> {
    busy = what
    try {
      return await fn()
    } catch (e) {
      toastError(failed, e)
      return null
    } finally {
      busy = null
      void qc.invalidateQueries({ queryKey: syncKey(board.id) })
    }
  }

  /** One edit of the connection; true when the server took it. */
  async function update(input: UpdateInput, what: string, failed: string): Promise<boolean> {
    const r = await act(what, () => updateBoardSync(board.id, input), failed)
    if (r) put(r)
    return !!r
  }

  async function saveMapping() {
    if (await update({ statusMap: statusMapOf(draft, targets) }, 'mapping', 'Could not save the mapping')) {
      pushToast({ title: 'Mapping saved', body: 'Tickets the old mapping placed are moving; a ticket someone moved stays put.', tone: 'success' })
    }
  }

  const setOption = (key: keyof SyncOptions, value: boolean) => {
    const options: Partial<SyncOptions> = {}
    options[key] = value
    return update({ options }, `option:${key}`, 'Could not change what carries over')
  }

  const setCadence = (id: CadenceId) =>
    update({ intervalMinutes: cadenceMinutes(id) }, 'cadence', 'Could not change the schedule')

  async function setPaused(pause: boolean) {
    if (await update({ paused: pause }, 'pause', pause ? 'Could not pause the sync' : 'Could not resume the sync')) {
      pushToast(
        pause
          ? { title: 'Sync paused', body: 'Nothing syncs from ClickUp until you resume it — not the schedule, Sync now or a ticket’s re-sync.', tone: 'success' }
          : { title: 'Sync resumed', body: 'The schedule is back on, and Sync now works again.', tone: 'success' },
      )
    }
  }

  async function runNow() {
    const r = await act('now', () => syncNow(board.id), 'Could not start a sync')
    if (r?.live) pushToast({ title: 'A sync is already running', body: 'Its progress shows here and on the board.', tone: 'info' })
  }

  async function replaceToken() {
    const t = token.trim()
    const r = await act('credential', () => replaceSyncCredential(board.id, t), 'Could not replace the credential')
    if (!r) return
    token = ''
    put(r)
    pushToast({ title: 'Credential replaced', body: `The sync now reads ClickUp as ${accountLabel(r.sync.account)}.`, tone: 'success' })
  }

  async function disconnect() {
    const ok = await confirm({
      title: 'Disconnect from ClickUp?',
      message: 'Stops future syncs. Every imported ticket stays on this board, editable, owned by Talaria.',
      confirmLabel: 'Disconnect',
      danger: true,
    })
    if (!ok) return
    const r = await act('disconnect', () => disconnectBoardSync(board.id), 'Could not disconnect')
    if (r) pushToast({ title: 'Disconnected from ClickUp', body: 'The imported tickets stay on this board.', tone: 'success' })
  }

  const label = 'mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim'
  const OPTION_ROWS: Array<{ key: keyof SyncOptions; label: string; hint: string }> = [
    { key: 'comments', label: 'Comments', hint: 'Into the ticket’s discussion, attributed to their ClickUp author by name.' },
    { key: 'attachments', label: 'Attachments', hint: 'Files up to 25 MB are copied; larger ones stay in ClickUp and are listed.' },
    { key: 'includeClosed', label: 'Import closed tasks', hint: 'Make tickets for tasks already closed in ClickUp. A synced ticket always follows its task when it closes.' },
    { key: 'notifyApps', label: 'Tell apps about synced tickets', hint: 'Apps on this board hear ticket.created and ticket.moved for the sync’s writes.' },
  ]
  const fullTime = (iso: string | null) => (iso ? new Date(iso).toLocaleString() : undefined)
  const summary = $derived(lastRunNote(sync.lastRun?.result))
</script>

<div class="space-y-5" data-sync-connected data-sync-state={sync.state}>
  {#if sync.state === 'needs_credential'}
    <div role="alert" class="rounded-lg border border-danger/40 p-3" data-sync-needs-credential>
      <div class="font-sans text-sm font-medium text-danger">ClickUp refused the sync’s token</div>
      <div class="mt-0.5 font-sans text-xs text-muted">
        The token was revoked, or its ClickUp member was deactivated. Nothing syncs until {isOwner ? 'you paste' : 'a board owner pastes'}
        a new one; the mapping, the links and the history are kept.
      </div>
      {#if isOwner}
        <div class="mt-2 flex flex-wrap gap-2">
          <Input type="password" autocomplete="off" spellcheck={false} placeholder="pk_…" size="sm" class="min-w-0 flex-1 font-mono" aria-label="New ClickUp token" bind:value={token} />
          <Button size="sm" disabled={token.trim().length < 8 || !!busy} onclick={() => void replaceToken()}>
            {busy === 'credential' ? 'Checking…' : 'Replace credential'}
          </Button>
        </div>
      {/if}
    </div>
  {/if}

  <!-- STATUS: where it reads, as whom, when it ran and runs next. -->
  <section class="rounded-lg border border-line p-3">
    <div class="flex flex-wrap items-center gap-2">
      <StatusDot status={DOT_FOR_TONE[view.tone]} pulse={view.tone === 'live'} />
      <span class="font-sans text-sm font-medium text-fg" data-sync-headline>ClickUp · {view.text}</span>
      <span class="ml-auto flex items-center gap-2">
        {#if isOwner && sync.state !== 'needs_credential'}
          <Button
            variant={paused ? 'outline' : 'ghost'}
            size="sm"
            disabled={!!busy}
            title={paused ? undefined : 'Freeze the sync: no scheduled sync, no Sync now and no ticket re-sync until you resume it'}
            data-sync-pause={paused ? 'resume' : 'pause'}
            onclick={() => void setPaused(!paused)}
          >
            {busy === 'pause' ? (paused ? 'Resuming…' : 'Pausing…') : paused ? 'Resume' : 'Pause'}
          </Button>
        {/if}
        {#if canSync}
          <Button
            variant="outline"
            size="sm"
            disabled={!!busy || !!sync.liveRun || sync.state === 'needs_credential' || paused}
            title={sync.state === 'needs_credential' ? 'The sync needs a new token first' : paused ? pausedHint(sync, clock.now, isOwner) : undefined}
            onclick={() => void runNow()}
          >
            {sync.liveRun ? 'Syncing…' : 'Sync now'}
          </Button>
        {/if}
      </span>
    </div>
    <div class="mt-1 font-sans text-xs text-muted" data-sync-detail>{view.detail}</div>

    <dl class="mt-3 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 font-sans text-[13px]">
      <dt class="text-muted">List</dt>
      <dd class="min-w-0 truncate text-fg">
        {#if sync.listUrl}
          <a href={sync.listUrl} target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 underline-offset-2 hover:underline">
            {sync.listPath ?? sync.listId} <ExternalLink size={11} class="shrink-0 text-muted" />
          </a>
        {:else}
          {sync.listPath ?? sync.listId}
        {/if}
      </dd>
      <dt class="text-muted">Reads as</dt>
      <dd class="min-w-0 truncate text-fg">
        {accountLabel(sync.account)}{sync.credentialHint ? ` · token ending ${sync.credentialHint}` : ''}
      </dd>
      {#if sync.connectedBy}
        <dt class="text-muted">Connected by</dt>
        <dd class="min-w-0 truncate text-fg">
          {sync.connectedBy.name ?? sync.connectedBy.email ?? 'someone'}
          <span class="text-muted" title={fullTime(sync.createdAt)}>· {ago(sync.createdAt, clock.now)}</span>
        </dd>
      {/if}
      <dt class="text-muted">Last sync</dt>
      <dd class="text-fg" title={fullTime(sync.lastSyncedAt)}>
        {ago(sync.lastSyncedAt, clock.now) ?? 'not yet'}{summary ? ` — ${summary}` : ''}
      </dd>
      <dt class="text-muted">Next sync</dt>
      <dd class="text-fg" title={fullTime(sync.nextSyncAt)}>
        {#if paused}none while paused{:else if sync.intervalMinutes == null}only on Sync now{:else if !sync.schedulerArmed}scheduled syncs are off on this instance{:else}{until(sync.nextSyncAt, clock.now) ?? '—'}{/if}
      </dd>
      {#if sync.liveRun}
        <dt class="text-muted">Running</dt>
        <dd class="text-fg" data-sync-phase>{sync.liveRun.phase || 'Starting'}</dd>
      {/if}
    </dl>
  </section>

  {#each sync.warnings as w (w.kind)}
    <div class="rounded-lg border border-warning/40 px-3 py-2" data-sync-warning={w.kind}>
      <div class="font-sans text-[13px] text-fg">{w.detail}</div>
      <div class="font-sans text-xs text-muted">{w.fix}</div>
    </div>
  {/each}

  <section>
    <div class={label}>Problems{sync.problemCount ? ` (${sync.problemCount})` : ''}</div>
    {#if sync.problems.length}
      <SyncProblems problems={sync.problems} total={sync.problemCount} boardId={board.id} />
    {:else}
      <div class="font-sans text-xs text-muted">None — every task the sync read landed as it should.</div>
    {/if}
  </section>

  <section>
    <div class={label}>Status mapping</div>
    <p class="mb-2 font-sans text-xs text-muted">
      {#if isOwner}
        Saving re-places the tickets the old mapping placed; a ticket someone has moved since stays where they put it.
      {:else}
        Only a board owner changes the mapping.
      {/if}
    </p>
    {#if rows.length}
      <SyncMappingTable {rows} {targets} {intake} bind:draft disabled={!isOwner} />
    {:else}
      <div class="font-sans text-xs text-muted">The list’s statuses arrive with the first sync.</div>
    {/if}
    {#if isOwner}
      <div class="mt-2 flex items-center justify-end gap-3">
        {#if sync.remapPending}<span class="font-sans text-xs text-muted" data-sync-remap>Moving tickets to the new mapping…</span>{/if}
        {#if dirty}
          <Button variant="ghost" size="sm" disabled={!!busy} onclick={() => (draft = { ...sync.statusMap })}>Discard</Button>
        {/if}
        <Button size="sm" disabled={!dirty || !!busy} onclick={() => void saveMapping()}>
          {busy === 'mapping' ? 'Saving…' : 'Save mapping'}
        </Button>
      </div>
    {/if}
  </section>

  <section>
    <div class={label}>What carries over</div>
    <div class="space-y-2">
      {#each OPTION_ROWS as o (o.key)}
        <div class="flex items-start gap-3">
          <Toggle checked={sync.options[o.key]} disabled={!isOwner || !!busy} onChange={(v) => void setOption(o.key, v)} title={o.label} />
          <div class="min-w-0">
            <div class="font-sans text-sm text-fg">{o.label}</div>
            <div class="font-sans text-[11px] text-muted">{o.hint}</div>
          </div>
        </div>
      {/each}
    </div>
  </section>

  <section>
    <div class={label}>Schedule</div>
    {#if isOwner}
      <Segmented
        options={CADENCES.map((c) => ({ id: c.id, label: c.label, data: { 'data-cadence': c.id } }))}
        value={cadenceId(sync.intervalMinutes)}
        onChange={(id) => void setCadence(id)}
      />
    {:else}
      <div class="font-sans text-sm text-fg">{cadenceLabel(sync.intervalMinutes)}</div>
    {/if}
  </section>

  {#if isOwner}
    {#if sync.state !== 'needs_credential'}
      <Disclosure title="Replace credential">
        <p class="mb-2 font-sans text-xs text-muted">
          When the member whose token this is leaves ClickUp, or you move to a dedicated integration member. The new token
          must be able to see this list; the mapping, the links and the history are kept.
        </p>
        <div class="flex flex-wrap gap-2">
          <Input type="password" autocomplete="off" spellcheck={false} placeholder="pk_…" size="sm" class="min-w-0 flex-1 font-mono" aria-label="New ClickUp token" bind:value={token} />
          <Button variant="outline" size="sm" disabled={token.trim().length < 8 || !!busy} onclick={() => void replaceToken()}>
            {busy === 'credential' ? 'Checking…' : 'Replace'}
          </Button>
        </div>
      </Disclosure>
    {/if}

    <div class="rounded-lg border border-danger/40 p-3">
      <div class="mb-2 flex items-center font-mono text-[10px] uppercase tracking-[0.08em]">
        <span class="text-danger">Danger zone</span>
      </div>
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <div class="font-sans text-sm text-fg">Disconnect from ClickUp</div>
          <div class="font-sans text-xs text-muted">Stops future syncs. Every imported ticket stays here, editable, owned by Talaria.</div>
        </div>
        <DangerLink class="shrink-0" disabled={!!busy} onClick={() => void disconnect()}>Disconnect</DangerLink>
      </div>
    </div>
  {:else}
    <p class="font-sans text-xs text-muted">
      Only a board owner changes the mapping, the schedule or the credential, pauses or resumes, or disconnects. You can Sync
      now and re-sync a ticket from its page, except while the sync is paused.
    </p>
  {/if}
  {#if busy && busy !== 'now'}<span class="sr-only" aria-live="polite">Saving</span>{/if}
</div>
