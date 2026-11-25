<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ExternalLink } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Steps from '@/components/ui/Steps.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import type { Board } from '@/lib/boards.svelte'
  import {
    CADENCES,
    DEFAULT_CADENCE,
    DEFAULT_OPTIONS,
    PERSON_OUTCOME,
    accountLabel,
    browseClickUp,
    cadenceMinutes,
    connectClickUp,
    landsIn,
    previewClickUp,
    statusKey,
    statusMapOf,
    syncKey,
    type BrowseLevel,
    type CadenceId,
    type ClickUpAccount,
    type ClickUpListRef,
    type ClickUpNamed,
    type MappingRow,
    type SyncOptions,
    type SyncPreview,
  } from '@/lib/board-sync'
  import { errorMessage } from '@/lib/fetch-json'
  import { formatBytes } from '@/lib/format'
  import { pushToast } from '@/lib/toast.svelte'
  import SyncMappingTable from './SyncMappingTable.svelte'

  // CONNECTING A CLICKUP LIST — a board owner's three steps, nothing written
  // until the last button: (1) a ClickUp personal token, checked by reading
  // the account it acts as; (2) the list, picked one ClickUp level at a time;
  // (3) the preview — the status mapping (exact label matches pre-filled,
  // everything else unmapped), who would be assigned, the first tasks as they
  // would land, the labels it would create, oversized files, and how long the
  // first import should take — then connect, which seals the token and starts
  // the first sync. The token lives in this component's memory only, until
  // the connect call carries it; the server never answers it back.
  let { board }: { board: Board } = $props()

  const qc = useQueryClient()
  const STEPS = ['Token', 'List', 'Preview'] as const
  let step = $state(0)
  let token = $state('')
  let busy = $state<string | null>(null)
  let error = $state<string | null>(null)

  let account = $state<ClickUpAccount | null>(null)
  let workspaces = $state<ClickUpNamed[]>([])
  let spaces = $state<ClickUpNamed[]>([])
  let folders = $state<ClickUpNamed[]>([])
  let spaceLists = $state<ClickUpListRef[]>([])
  let folderLists = $state<ClickUpListRef[]>([])
  let teamId = $state('')
  let spaceId = $state('')
  /** '' = the lists outside any folder. */
  let folderId = $state('')
  let listId = $state('')
  const lists = $derived(folderId ? folderLists : spaceLists)

  let preview = $state<SyncPreview | null>(null)
  let draft = $state<Record<string, string>>({})
  let options = $state<SyncOptions>({ ...DEFAULT_OPTIONS })
  let cadence = $state<CadenceId>(DEFAULT_CADENCE)

  const intake = $derived(preview ? (preview.columns.find((c) => c.key === preview!.intake) ?? null) : null)
  const rows = $derived<MappingRow[]>(
    (preview?.statuses ?? []).map((s) => ({ name: s.name, key: statusKey(s.name), color: s.color, type: s.type, stale: false })),
  )
  const columnLabel = (key: string | null) =>
    key ? (preview?.columns.find((c) => c.key === key)?.label ?? key) : 'refused — no intake column'
  const unmappedCount = $derived(rows.filter((r) => !draft[r.key]).length)

  /** One call, one sentence on failure: the server's own refusal. */
  async function run<T>(what: string, fn: () => Promise<T>): Promise<T | null> {
    busy = what
    error = null
    try {
      return await fn()
    } catch (e) {
      error = errorMessage(e)
      return null
    } finally {
      busy = null
    }
  }
  const browse = (input: Parameters<typeof browseClickUp>[1]) =>
    run('browse', () => browseClickUp(board.id, { token: token.trim(), ...input }))

  async function checkToken() {
    const r = await browse({})
    if (!r) return
    account = r.account
    workspaces = r.workspaces ?? []
    reset('team')
    step = 1
    // One workspace (the usual case): walk into it without asking.
    if (workspaces.length === 1) await pickWorkspace(workspaces[0]!.id)
  }

  /** Clear every pick below a level: a new workspace empties its spaces, a
   *  new space its folders and lists, a new folder its lists. */
  function reset(from: 'team' | 'space' | 'folder') {
    if (from === 'team') {
      teamId = ''
      spaces = []
    }
    if (from !== 'folder') {
      spaceId = ''
      folders = []
      spaceLists = []
    }
    folderId = ''
    folderLists = []
    listId = ''
  }

  async function pickWorkspace(id: string) {
    reset('team')
    teamId = id
    if (!id) return
    const r: BrowseLevel | null = await browse({ teamId: id })
    if (r) {
      spaces = r.spaces ?? []
      if (spaces.length === 1) await pickSpace(spaces[0]!.id)
    }
  }

  async function pickSpace(id: string) {
    reset('space')
    spaceId = id
    if (!id) return
    const r = await browse({ spaceId: id })
    if (r) {
      folders = r.folders ?? []
      spaceLists = r.lists ?? []
    }
  }

  async function pickFolder(id: string) {
    reset('folder')
    folderId = id
    if (!id) return
    const r = await browse({ folderId: id })
    if (r) folderLists = r.lists ?? []
  }

  async function loadPreview() {
    const r = await run('preview', () => previewClickUp(board.id, { token: token.trim(), listId }))
    if (!r) return
    preview = r
    // The server's suggestions: exact label matches, or this board's last
    // table for the same list. Everything else starts unmapped.
    draft = Object.fromEntries(r.statuses.map((s) => [statusKey(s.name), s.suggested ?? '']))
    step = 2
  }

  async function connect() {
    if (!preview) return
    const r = await run('connect', () =>
      connectClickUp(board.id, {
        token: token.trim(),
        listId,
        statusMap: statusMapOf(draft, preview!.columns),
        options,
        intervalMinutes: cadenceMinutes(cadence),
      }),
    )
    if (!r) return
    token = ''
    qc.setQueryData(syncKey(board.id), { sync: r.sync, disconnected: null })
    void qc.invalidateQueries({ queryKey: syncKey(board.id) })
    pushToast({ title: 'Connected to ClickUp', body: `${preview.list.path} — the first sync is running.`, tone: 'success' })
  }

  const label = 'mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim'
  const OPTION_ROWS: Array<{ key: keyof SyncOptions; label: string; hint: string }> = [
    { key: 'comments', label: 'Comments', hint: 'Into the ticket’s discussion, attributed to their ClickUp author by name.' },
    { key: 'attachments', label: 'Attachments', hint: 'Files up to 25 MB are copied; larger ones stay in ClickUp and are listed.' },
    { key: 'includeClosed', label: 'Import closed tasks', hint: 'Make tickets for tasks already closed in ClickUp.' },
    {
      key: 'notifyApps',
      label: 'Tell apps about synced tickets',
      hint: 'Off by default: a first import would run a new-ticket app once per ticket. Turn it on after the first import if an app should follow ClickUp.',
    },
  ]
</script>

<div class="space-y-5" data-sync-setup>
  <div class="flex flex-wrap items-center justify-between gap-3">
    <Steps steps={STEPS} current={step} />
    {#if account && step > 0}
      <span class="font-sans text-xs text-muted">Reads ClickUp as {accountLabel(account)}</span>
    {/if}
  </div>

  {#if step === 0}
    <div class="space-y-3">
      <p class="font-sans text-sm text-muted">
        Sync this board from one ClickUp list: each task becomes a ticket here, and keeps following ClickUp on a schedule
        and on Sync now. One way — nothing is written back to ClickUp.
      </p>
      <div>
        <label class={label} for="clickup-token">
          ClickUp personal token
          <InfoTip
            class="ml-1"
            text="The sync reads ClickUp as the member this token belongs to, so it sees only what they can see. A token stops working when its member is deactivated — use a dedicated ClickUp member for integrations if you can."
          />
        </label>
        <Input
          id="clickup-token"
          type="password"
          autocomplete="off"
          spellcheck={false}
          placeholder="pk_…"
          class="w-full font-mono"
          bind:value={token}
          onkeydown={(e) => e.key === 'Enter' && token.trim().length >= 8 && !busy && void checkToken()}
        />
        <div class="mt-1 font-sans text-[11px] text-muted">
          Make one in ClickUp under
          <a
            href="https://app.clickup.com/settings/apps"
            target="_blank"
            rel="noopener noreferrer"
            class="inline-flex items-center gap-0.5 text-accent underline-offset-2 hover:underline"
          >
            Settings → Apps <ExternalLink size={10} />
          </a>. It is sealed on the board and never shown again — only its last four characters.
        </div>
      </div>
      <div class="flex justify-end">
        <Button size="sm" disabled={token.trim().length < 8 || !!busy} onclick={() => void checkToken()}>
          {busy === 'browse' ? 'Checking…' : 'Continue'}
        </Button>
      </div>
    </div>
  {:else if step === 1}
    <div class="space-y-3">
      <div class="grid gap-3 sm:grid-cols-2">
        <div>
          <label class={label} for="clickup-workspace">Workspace</label>
          <Select id="clickup-workspace" size="sm" class="w-full" value={teamId} disabled={!!busy} onchange={(e) => void pickWorkspace(e.currentTarget.value)}>
            <option value="">Pick a workspace</option>
            {#each workspaces as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
          </Select>
        </div>
        <div>
          <label class={label} for="clickup-space">Space</label>
          <Select id="clickup-space" size="sm" class="w-full" value={spaceId} disabled={!teamId || !!busy} onchange={(e) => void pickSpace(e.currentTarget.value)}>
            <option value="">Pick a space</option>
            {#each spaces as s (s.id)}<option value={s.id}>{s.name}</option>{/each}
          </Select>
        </div>
        <div>
          <label class={label} for="clickup-folder">Folder</label>
          <Select id="clickup-folder" size="sm" class="w-full" value={folderId} disabled={!spaceId || !!busy} onchange={(e) => void pickFolder(e.currentTarget.value)}>
            <option value="">Lists outside folders</option>
            {#each folders as f (f.id)}<option value={f.id}>{f.name}</option>{/each}
          </Select>
        </div>
        <div>
          <label class={label} for="clickup-list">List</label>
          <Select id="clickup-list" size="sm" class="w-full" value={listId} disabled={!spaceId || !!busy} onchange={(e) => (listId = e.currentTarget.value)}>
            <option value="">{spaceId && lists.length === 0 && !busy ? 'No lists here' : 'Pick a list'}</option>
            {#each lists as l (l.id)}
              <option value={l.id}>{l.name}{l.taskCount != null ? ` (${l.taskCount} tasks)` : ''}</option>
            {/each}
          </Select>
        </div>
      </div>
      <div class="flex justify-between gap-2">
        <Button variant="ghost" size="sm" disabled={!!busy} onclick={() => (step = 0)}>Back</Button>
        <Button size="sm" disabled={!listId || !!busy} onclick={() => void loadPreview()}>
          {busy === 'preview' ? 'Reading the list…' : busy ? 'Reading ClickUp…' : 'Preview'}
        </Button>
      </div>
    </div>
  {:else if preview}
    <div class="space-y-5" data-sync-preview>
      <div>
        <div class="font-sans text-sm text-fg">{preview.list.path}</div>
        <div class="font-sans text-xs text-muted">
          {preview.list.taskCount ?? preview.statsFrom.tasks} tasks{preview.statsFrom.partial
            ? ` — the figures below come from the first ${preview.statsFrom.tasks}`
            : ''}. Nothing is written until you connect.
        </div>
      </div>

      <section>
        <div class={label}>Status mapping</div>
        <p class="mb-2 font-sans text-xs text-muted">
          Exact name matches are filled in; nothing else is guessed. An unmapped status lands in
          {intake ? intake.label : 'no column (this board has no intake column without agent pickup)'} and is listed as a
          problem until you map it.
        </p>
        <SyncMappingTable {rows} targets={preview.columns} {intake} bind:draft />
        {#if unmappedCount > 0}
          <div class="mt-1.5 font-sans text-xs text-warning">{unmappedCount} unmapped status{unmappedCount === 1 ? '' : 'es'}.</div>
        {/if}
      </section>

      {#if preview.sample.length > 0}
        <section>
          <div class={label}>The first {preview.sample.length} tasks, as they would land</div>
          <ul class="divide-y divide-line-subtle rounded-lg border border-line" data-sync-sample>
            {#each preview.sample as t (t.id)}
              {@const lands = landsIn(t.clickupStatus, draft, preview.intake)}
              <li class="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-1.5">
                <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg" title={t.title}>{t.title}</span>
                <span class="font-sans text-xs text-muted">
                  {t.clickupStatus} → <span class={lands.unmapped ? 'text-warning' : 'text-fg'}>{columnLabel(lands.column)}</span>{lands.unmapped
                    ? ' (unmapped)'
                    : ''}
                </span>
                <Chip>{t.priority}</Chip>
                {#if t.closed}<Chip title="Closed in ClickUp">closed</Chip>{/if}
                {#if t.assignees.length}<span class="font-sans text-xs text-muted">{t.assignees.length} assigned</span>{/if}
              </li>
            {/each}
          </ul>
        </section>
      {/if}

      {#if preview.people.length > 0}
        <section>
          <div class={label}>Assignees, matched by email</div>
          <ul class="space-y-1" data-sync-people>
            {#each preview.people as p (p.email ?? p.clickupName)}
              <li class="flex flex-wrap items-center gap-2 font-sans text-sm">
                <span class="text-fg">{p.clickupName}</span>
                {#if p.email}<span class="text-xs text-muted">{p.email}</span>{/if}
                <Chip tone={PERSON_OUTCOME[p.outcome].tone}>{PERSON_OUTCOME[p.outcome].label}</Chip>
              </li>
            {/each}
          </ul>
        </section>
      {/if}

      <section class="grid gap-3 sm:grid-cols-2">
        <div>
          <div class={label}>New labels</div>
          {#if preview.newLabels.length}
            <div class="flex flex-wrap gap-1">{#each preview.newLabels as l (l)}<Chip>{l}</Chip>{/each}</div>
          {:else}
            <div class="font-sans text-xs text-muted">None — every tag is already a label here.</div>
          {/if}
        </div>
        <div>
          <div class={label}>First import</div>
          <div class="font-sans text-xs text-muted" data-sync-estimate>
            About {preview.estimate.minutes} minute{preview.estimate.minutes === 1 ? '' : 's'} at {preview.estimate.perMinute} requests a
            minute ({preview.estimate.requests} requests){preview.estimate.bytes > 0
              ? `, and about ${formatBytes(preview.estimate.bytes)} of files`
              : ''}.
            {#if preview.oversizeInSample > 0}
              {preview.oversizeInSample} file{preview.oversizeInSample === 1 ? '' : 's'} in the sample {preview.oversizeInSample === 1
                ? 'is'
                : 'are'} over 25 MB and will stay in ClickUp, listed as problems.
            {/if}
          </div>
        </div>
      </section>

      <section>
        <div class={label}>What carries over</div>
        <div class="space-y-2">
          {#each OPTION_ROWS as o (o.key)}
            <div class="flex items-start gap-3">
              <Toggle checked={options[o.key]} onChange={(v) => (options = { ...options, [o.key]: v })} title={o.label} />
              <div class="min-w-0">
                <div class="font-sans text-sm text-fg">
                  {o.label}{o.key === 'includeClosed' && preview.closedTasks > 0 ? ` (${preview.closedTasks} closed)` : ''}
                </div>
                <div class="font-sans text-[11px] text-muted">{o.hint}</div>
              </div>
            </div>
          {/each}
        </div>
      </section>

      <section>
        <div class={label}>Sync</div>
        <Segmented
          options={CADENCES.map((c) => ({ id: c.id, label: c.label, data: { 'data-cadence': c.id } }))}
          value={cadence}
          onChange={(id) => (cadence = id)}
        />
        <div class="mt-1 font-sans text-[11px] text-muted">Sync now works whichever you pick; Manual syncs only then.</div>
      </section>

      <div class="flex justify-between gap-2">
        <Button variant="ghost" size="sm" disabled={!!busy} onclick={() => (step = 1)}>Back</Button>
        <Button size="sm" disabled={!!busy} onclick={() => void connect()}>
          {busy === 'connect' ? 'Connecting…' : 'Connect and start syncing'}
        </Button>
      </div>
    </div>
  {/if}

  {#if error}
    <div role="alert" class="rounded-md border border-danger/40 px-3 py-2 font-sans text-sm text-danger" data-sync-error>{error}</div>
  {/if}
</div>
