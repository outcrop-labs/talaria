<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowFix from '@/components/settings/SettingsRowFix.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Select from '@/components/ui/Select.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { errorMessage, postJson, putJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import NoticeRow from './NoticeRow.svelte'
  import RowStatus from './RowStatus.svelte'
  import TargetFields from './TargetFields.svelte'
  import ToggleField from './ToggleField.svelte'
  import { fmtBytes, useStorageAdmin, type StorageAdmin } from './storage'
  import { sansFill } from '@/lib/ui-voice'

  // Where upload blobs live: local disk, the bundled MinIO container ("built-in
  // bucket"), or any external S3-compatible service — plus an optional replica
  // that mirrors every blob to a second provider.
  type Config = StorageAdmin['config']
  type Part = 'location' | 'replica'

  const qc = useQueryClient()
  const query = useStorageAdmin()
  const data = $derived(query.data)

  // The draft a dialog edits. Seeded from the saved config each time one
  // opens, so closing a dialog drops what was typed in it.
  let editing = $state<Part | null>(null)
  let form = $state<Config | null>(null)
  let secret = $state('')
  let replicaSecret = $state('')
  let busy = $state(false)
  let saveError = $state<string | null>(null)
  let note = $state<{ part: Part; ok: boolean; text: string } | null>(null)

  const edit = (part: Part, patch?: Partial<Config['replica']>) => {
    if (!data) return
    form = { ...data.config, replica: { ...data.config.replica, ...patch } }
    secret = ''
    replicaSecret = ''
    saveError = null
    editing = part
  }
  const set = (patch: Partial<Config>) => {
    if (form) form = { ...form, ...patch }
  }
  const setReplica = (patch: Partial<Config['replica']>) => {
    if (form) form = { ...form, replica: { ...form.replica, ...patch } }
  }

  const save = async () => {
    if (!form || !editing) return
    const part = editing
    busy = true
    saveError = null
    try {
      await putJson<{ config: Config }>('/api/admin/storage', {
        ...form,
        secretAccessKey: secret || undefined,
        replica: { ...form.replica, secretAccessKey: replicaSecret || undefined },
      })
      editing = null
      note = { part, ok: true, text: 'Saved.' }
      await qc.invalidateQueries({ queryKey: ['storage-admin'] })
    } catch (e) {
      saveError = errorMessage(e)
    } finally {
      busy = false
    }
  }

  const act = async (action: 'test' | 'test-replica' | 'migrate' | 'sync') => {
    busy = true
    const part: Part = action === 'test' || action === 'migrate' ? 'location' : 'replica'
    if (action === 'test' || action === 'test-replica') note = null
    try {
      if (action === 'test' || action === 'test-replica') {
        // The verdict rides a 200 body ({ ok, detail }); a server-side failure
        // is a 4xx the door rejects into the catch below.
        const j = await postJson<{ ok: boolean; detail: string }>('/api/admin/storage', { action })
        note = { part, ok: j.ok, text: j.detail }
      } else {
        await postJson('/api/admin/storage', { action })
      }
    } catch (e) {
      note = { part, ok: false, text: errorMessage(e) }
    } finally {
      busy = false
    }
    // Refetch either way: a failed migrate/sync may still have moved blobs,
    // and the rows above should say so.
    await qc.invalidateQueries({ queryKey: ['storage-admin'] })
  }

  const migrate = async () => {
    if (!data) return
    const ok = await confirm({
      title: 'Move local files to the bucket?',
      message: `Copies ${data.stats.local} file${data.stats.local === 1 ? '' : 's'} (${fmtBytes(data.stats.localBytes)}) into the active bucket and repoints each record. Local copies are left on disk.`,
      confirmLabel: 'Move files',
    })
    if (ok) await act('migrate')
  }

  const files = (n: number) => `${n.toLocaleString()} file${n === 1 ? '' : 's'}`

  // A bucket is named by itself and found at its endpoint. The rest of what
  // describes it (region, prefix, keys) is in its dialog.
  const bucketOf = (d: StorageAdmin): { bucket: string; endpoint: string } | null =>
    d.config.mode === 's3' ? d.config : d.config.mode === 'internal' ? d.internal : null
</script>

<!-- A bucket is its name and the endpoint it is found at. -->
{#snippet bucketLine(bucket: string | undefined, endpoint: string | undefined)}
  <span class="break-all font-mono text-xs">{bucket || 'No bucket set'}</span>{' · '}<span class="break-all font-mono text-xs">{endpoint || 'No endpoint set'}</span>
{/snippet}

{#snippet result(part: Part)}
  {#if note?.part === part}
    <div transition:slide={{ duration: 150 }}>
      <NoticeRow status={note.ok ? 'ok' : 'danger'}>{note.text}</NoticeRow>
    </div>
  {/if}
{/snippet}

<!-- A failed read used to leave `data` undefined for ever, so this panel
     shimmered its skeleton at a request that had already died — a loading
     state that never ends is just a slower blank screen. QueryState's error
     branch owns that. -->
<QueryState {query} errorTitle="Could not load storage settings">
  {#snippet skeleton()}
    <SettingsSection title="Stored files">
      <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={4} /></div></SettingsGroup>
    </SettingsSection>
  {/snippet}
  {#snippet children(d)}
    {@const saved = d.config}
    {@const inBucket = saved.mode !== 'local'}
    {@const external = saved.mode === 's3'}
    {@const migrating = d.migrate?.running}
    {@const syncing = d.sync?.running}
    {@const total = d.stats.local + d.stats.s3 + d.stats.internal}
    {@const places = [
      // The dashboards' series family: gold and coral read as "needs attention" and "failed" beside a status dot.
      { label: 'External storage', count: d.stats.s3, bar: 'bg-chart-1' },
      { label: 'Built-in bucket', count: d.stats.internal, bar: 'bg-fg' },
      { label: `Local disk (${fmtBytes(d.stats.localBytes)})`, count: d.stats.local, bar: 'bg-line-strong' },
    ]}
    <div class="space-y-8">
      <SettingsSection title="Stored files" description="Every uploaded file, by where it is kept.">
        <SettingsGroup>
          <div class="px-5 py-4">
            <div class="flex items-baseline justify-between gap-4">
              <span class="font-sans text-sm font-medium leading-5 text-fg">Files</span>
              <span class="font-sans text-sm leading-5 tabular-nums text-fg">{total.toLocaleString()}</span>
            </div>
            <div class="mt-3 flex h-1.5 overflow-hidden rounded-full {sansFill}" role="img" aria-label="Share of files by location">
              {#each places as p (p.label)}
                {#if p.count > 0}<div class={p.bar} style:width="{(p.count / total) * 100}%"></div>{/if}
              {/each}
            </div>
            <ul class="mt-3 flex flex-wrap gap-x-8 gap-y-1">
              {#each places as p (p.label)}
                <li class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
                  <span class="h-1.5 w-1.5 shrink-0 rounded-full {p.bar}"></span>
                  {p.label}
                  <span class="tabular-nums text-fg">{p.count.toLocaleString()}</span>
                </li>
              {/each}
            </ul>
          </div>
          {#if migrating && d.migrate}
            <SettingsRow label="Moving files to the bucket">
              {#snippet lead()}<WaitingMark site="admin/storage-migrate" size={14} />{/snippet}
              <span class="font-sans text-[13px] tabular-nums text-muted">{d.migrate.moved.toLocaleString()} of {d.migrate.total.toLocaleString()}</span>
            </SettingsRow>
          {/if}
          {#if inBucket && d.stats.local > 0 && !migrating}
            <NoticeRow status={d.migrate?.failed ? 'danger' : 'warn'}>
              {#if d.migrate?.failed}{files(d.migrate.failed)} failed to move.{/if}
              {files(d.stats.local)} {d.stats.local === 1 ? 'is' : 'are'} still on local disk.
              {#snippet action()}
                <SettingsRowFix label="Fix files on local disk: move them to the bucket" onclick={() => void migrate()} disabled={busy} />
              {/snippet}
            </NoticeRow>
          {:else if !migrating && d.migrate?.failed}
            <NoticeRow status="danger">{files(d.migrate.failed)} failed to move.</NoticeRow>
          {/if}
        </SettingsGroup>
      </SettingsSection>

      <SettingsSection title="Where files live" description="New uploads go to the primary, and a replica receives a copy of every file.">
        <SettingsGroup>
          <SettingsRow
            label="Primary"
            info="Local disk keeps everything on this machine. The built-in bucket is Talaria's own bundled object store, with no cloud account needed. External works with any S3-compatible service: AWS S3, Backblaze B2, Cloudflare R2, MinIO. Switching never breaks an existing link."
            onOpen={() => edit('location')}
            openLabel="Edit where files live"
          >
            {#snippet hint()}
              {#if inBucket}
                {#if !external}Built-in bucket{' · '}{/if}{@render bucketLine(bucketOf(d)?.bucket, bucketOf(d)?.endpoint)}
              {:else}
                Local disk, so files are kept on this machine.
              {/if}
            {/snippet}
            {#snippet status()}
              {#if external && !saved.hasSecret}<RowStatus status="warn">Secret key not set</RowStatus>{/if}
            {/snippet}
            {#if external && !saved.hasSecret}
              <SettingsRowFix label="Fix the primary storage: enter its secret access key" onclick={() => edit('location')} />
            {/if}
            <SettingsRowMenu
              label="Actions for file storage"
              items={[
                { label: 'Edit', onSelect: () => edit('location') },
                ...(inBucket
                  ? [{ label: 'Test connection', disabled: busy || (external && !saved.hasSecret), onSelect: () => void act('test') }]
                  : []),
                ...(inBucket && d.stats.local > 0 && !migrating
                  ? [{ label: 'Move local files to the bucket', disabled: busy, onSelect: () => void migrate() }]
                  : []),
              ]}
            />
          </SettingsRow>
          {@render result('location')}
          {#if saved.replica.enabled}
            {@const behind = !syncing && !!d.sync?.failed}
            {@const canSync = !busy && saved.replica.hasSecret && total > 0}
            <SettingsRow
              label="Replica"
              info="New uploads are copied as they land, and an outage at the replica never blocks an upload. Sync all copies everything already stored, on disk or in a bucket, into the replica bucket."
              onOpen={() => edit('replica')}
              openLabel="Edit the replica"
            >
              {#snippet hint()}{@render bucketLine(saved.replica.bucket, saved.replica.endpoint)}{/snippet}
              {#snippet status()}
                {#if syncing && d.sync}
                  <WaitingMark site="admin/storage-sync" size={14} />
                  <span class="tabular-nums">Syncing {d.sync.moved.toLocaleString()} of {d.sync.total.toLocaleString()}</span>
                {:else if !saved.replica.hasSecret}
                  <RowStatus status="warn">Secret key not set</RowStatus>
                {:else if behind && d.sync}
                  <RowStatus status="danger">{files(d.sync.failed)} not copied</RowStatus>
                {/if}
              {/snippet}
              {#if !saved.replica.hasSecret}
                <SettingsRowFix label="Fix the replica: enter its secret access key" onclick={() => edit('replica')} />
              {:else if behind}
                <SettingsRowFix label="Fix the replica: sync the files again" disabled={!canSync} onclick={() => void act('sync')} />
              {/if}
              <SettingsRowMenu
                label="Actions for the replica"
                items={[
                  { label: 'Edit', onSelect: () => edit('replica') },
                  { label: 'Test replica', disabled: busy || !saved.replica.hasSecret, onSelect: () => void act('test-replica') },
                  { label: 'Sync all to replica', disabled: !canSync || syncing, onSelect: () => void act('sync') },
                ]}
              />
            </SettingsRow>
          {:else}
            <!-- One place is a choice, not a fault, so the row opens the
                 setup and shows no button. -->
            <SettingsRow
              label="Replica"
              hint="Off, so files are kept in one place only."
              onOpen={() => edit('replica', { enabled: true })}
              openLabel="Set up a replica"
            >
              <SettingsRowMenu label="Actions for the replica" items={[{ label: 'Set up replica', onSelect: () => edit('replica', { enabled: true }) }]} />
            </SettingsRow>
            {#if !syncing && d.sync?.failed}
              <NoticeRow status="danger">{files(d.sync.failed)} did not reach the replica.</NoticeRow>
            {/if}
          {/if}
          {@render result('replica')}
        </SettingsGroup>
      </SettingsSection>
    </div>
  {/snippet}
</QueryState>

<Modal
  open={editing !== null && form !== null}
  onClose={() => (editing = null)}
  title={editing === 'replica' ? 'Replica' : 'Primary storage'}
  width="max-w-2xl"
>
  {#if form}
    {@const cfg = form}
    <div class="space-y-5">
      {#if editing === 'location'}
        <SettingsField label="Location" for="storage-mode">
          <Select id="storage-mode" size="sm" value={cfg.mode} onchange={(e) => set({ mode: e.currentTarget.value as Config['mode'] })} class="w-full">
            <option value="local">Local disk</option>
            <option value="internal">Built-in bucket</option>
            <option value="s3">External S3-compatible storage</option>
          </Select>
        </SettingsField>
        {#if cfg.mode === 'internal' && data}
          <p class="font-sans text-[13px] leading-5 text-muted">
            Uses <span class="font-mono text-xs text-fg">{data.internal.bucket}</span> at
            <span class="font-mono text-xs text-fg">{data.internal.endpoint}</span>. The bucket is created automatically, with
            credentials from <span class="font-mono text-xs">TALARIA_S3_*</span> in the environment.
          </p>
        {/if}
        {#if cfg.mode === 's3'}
          <TargetFields id="storage" t={cfg} {secret} onChange={set} onSecret={(v) => (secret = v)} />
        {/if}
      {:else}
        <ToggleField
          label="Replicate files"
          hint="Copy every file to this second provider."
          checked={cfg.replica.enabled}
          onChange={(checked) => setReplica({ enabled: checked })}
        />
        {#if cfg.replica.enabled}
          <div transition:slide={{ duration: 150 }}>
            <TargetFields id="replica" t={cfg.replica} secret={replicaSecret} onChange={setReplica} onSecret={(v) => (replicaSecret = v)} />
          </div>
        {/if}
      {/if}
      {#if saveError}<div class="font-sans text-[13px] leading-5 text-danger" role="alert">{saveError}</div>{/if}
    </div>
  {/if}
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button variant="ghost" size="sm" onclick={() => (editing = null)}>Cancel</Button>
      <Button size="sm" onclick={() => void save()} disabled={busy}>{busy ? 'Saving' : editing === 'replica' ? 'Save replica' : 'Save storage'}</Button>
    </div>
  {/snippet}
</Modal>
