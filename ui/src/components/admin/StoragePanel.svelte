<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Panel from '@/components/ui/Panel.svelte'
  import GeneratingBars from '@/components/ui/GeneratingBars.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Select from '@/components/ui/Select.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import TargetFields from './TargetFields.svelte'
  import { fmtBytes, useStorageAdmin, type StorageAdmin } from './storage'

  // Where upload blobs live: local disk, the bundled MinIO container ("built-in
  // bucket"), or any external S3-compatible service — plus an optional replica
  // that mirrors every blob to a second provider.
  const qc = useQueryClient()
  const query = useStorageAdmin()
  const data = $derived(query.data)
  let form = $state<StorageAdmin['config'] | null>(null)
  let secret = $state('')
  let replicaSecret = $state('')
  let busy = $state(false)
  let note = $state<{ ok: boolean; text: string } | null>(null)

  $effect(() => {
    if (data && !form) form = data.config
  })

  const set = (patch: Partial<StorageAdmin['config']>) => {
    if (form) form = { ...form, ...patch }
  }
  const setReplica = (patch: Partial<StorageAdmin['config']['replica']>) => {
    if (form) form = { ...form, replica: { ...form.replica, ...patch } }
  }

  const save = async () => {
    if (!form) return
    busy = true
    note = null
    try {
      const r = await fetch('/api/admin/storage', {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          ...form,
          secretAccessKey: secret || undefined,
          replica: { ...form.replica, secretAccessKey: replicaSecret || undefined },
        }),
      })
      const j = (await r.json()) as { config?: StorageAdmin['config']; error?: string }
      if (!r.ok || !j.config) {
        note = { ok: false, text: j.error ?? 'save failed' }
        return
      }
      form = j.config
      secret = ''
      replicaSecret = ''
      note = { ok: true, text: 'saved' }
      await qc.invalidateQueries({ queryKey: ['storage-admin'] })
    } finally {
      busy = false
    }
  }

  const act = async (action: 'test' | 'test-replica' | 'migrate' | 'sync') => {
    busy = true
    if (action === 'test' || action === 'test-replica') note = null
    try {
      const r = await fetch('/api/admin/storage', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action }) })
      const j = (await r.json()) as { ok?: boolean; detail?: string; error?: string }
      if (action === 'test' || action === 'test-replica') note = { ok: !!j.ok, text: j.detail ?? j.error ?? '' }
      else if (j.error) note = { ok: false, text: j.error }
      await qc.invalidateQueries({ queryKey: ['storage-admin'] })
    } finally {
      busy = false
    }
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

  const inBucket = $derived(form !== null && form.mode !== 'local')
  const external = $derived(form?.mode === 's3')
  const migrating = $derived(data?.migrate?.running)
  const syncing = $derived(data?.sync?.running)
  const totalBlobs = $derived(data ? data.stats.local + data.stats.s3 + data.stats.internal : 0)
</script>

<Panel>
  <!-- A failed read used to leave `data` undefined for ever, so this panel
       shimmered its skeleton at a request that had already died — a loading
       state that never ends is just a slower blank screen. -->
  {#if !data && query.isError}
    <QueryError error={query.error} title="Could not load storage settings" onRetry={() => void query.refetch()} />
  {:else if !data || !form}
    <!-- Panel-shaped skeleton (title, stat strip, config grid, button row) so
         the Storage tab never renders blank and then materializes. -->
    <Skeleton class="mb-4 h-4 w-20 rounded-full" />
    <div class="mb-3 flex flex-wrap items-center gap-3 rounded-md border border-line p-3">
      <Skeleton class="h-3 w-32 rounded-full" />
      <Skeleton class="h-3 w-36 rounded-full" delay={0.12} />
      <Skeleton class="h-3 w-32 rounded-full" delay={0.24} />
    </div>
    <div class="grid gap-2 sm:grid-cols-2">
      {#each Array.from({ length: 6 }) as _, i (i)}
        <div>
          <Skeleton class="mb-1.5 h-2.5 w-24 rounded-full" delay={i * 0.08} />
          <Skeleton class="h-9 w-full" delay={i * 0.08} />
        </div>
      {/each}
    </div>
    <div class="mt-3 flex items-center gap-2">
      <Skeleton class="h-7 w-16" />
      <Skeleton class="h-7 w-32" delay={0.12} />
    </div>
  {:else}
    {@const cfg = form}
    {@const d = data}
    <SectionHeader
      class="mb-4"
      title="Storage"
      info="Where uploaded files live. Local disk keeps everything on this machine; the built-in bucket is Talaria's own bundled object store (no cloud account needed); external works with any S3-compatible service — AWS S3, Backblaze B2, Cloudflare R2, MinIO. Each file remembers where it was stored, so switching never breaks existing links. A replica mirrors every file to a second provider for redundancy."
    />

    <div class="mb-3 flex flex-wrap items-center gap-3 rounded-md border border-line p-3 text-xs text-muted">
      <span><span class="font-mono text-fg">{d.stats.local}</span> on disk (<span class="font-mono">{fmtBytes(d.stats.localBytes)}</span>)</span>
      <span><span class="font-mono text-fg">{d.stats.internal}</span> in the built-in bucket</span>
      <span><span class="font-mono text-fg">{d.stats.s3}</span> in external storage</span>
      {#if migrating}
        <span class="flex items-center gap-1.5"><GeneratingBars bars={3} variant="breathe" step={0.2} /> moving <span class="font-mono">{d.migrate!.moved}/{d.migrate!.total}</span></span>
      {/if}
      {#if syncing}
        <span class="flex items-center gap-1.5"><GeneratingBars bars={3} variant="breathe" step={0.2} /> syncing <span class="font-mono">{d.sync!.moved}/{d.sync!.total}</span> to replica</span>
      {/if}
      {#if !migrating && d.migrate?.failed}<span class="text-danger">{d.migrate.failed} failed to move</span>{/if}
      {#if !syncing && d.sync?.failed}<span class="text-danger">{d.sync.failed} failed to sync</span>{/if}
      {#if inBucket && d.stats.local > 0 && !migrating}
        <Button size="sm" variant="outline" class="ml-auto" onclick={() => void migrate()} disabled={busy}>
          Move local files to bucket
        </Button>
      {/if}
    </div>

    <div class="grid gap-2 sm:grid-cols-2">
      <label class="text-xs text-muted">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Mode</span>
        <Select value={cfg.mode} onchange={(e) => set({ mode: e.currentTarget.value as StorageAdmin['config']['mode'] })} class="mt-1 w-full">
          <option value="local">Local disk</option>
          <option value="internal">Built-in bucket (bundled MinIO)</option>
          <option value="s3">External (S3-compatible)</option>
        </Select>
      </label>
      {#if cfg.mode === 'internal'}
        <div class="self-end pb-1 text-xs text-muted">
          <span class="font-mono text-[11px]">{d.internal.endpoint}</span> · bucket <span class="font-mono text-[11px] text-fg">{d.internal.bucket}</span>
          <span class="opacity-70"> — creds via TALARIA_S3_* env; bucket auto-created</span>
        </div>
      {/if}
      {#if external}<TargetFields t={cfg} {secret} onChange={set} onSecret={(v) => (secret = v)} />{/if}
    </div>

    <!-- Replica — mirror every blob to a second provider -->
    <div class="mt-4 rounded-md border border-line p-3">
      <label class="flex items-center gap-2 text-xs font-medium text-fg">
        <input type="checkbox" checked={cfg.replica.enabled} onchange={(e) => setReplica({ enabled: e.currentTarget.checked })} class="accent-accent" />
        Replicate to a second provider
      </label>
      <p class="mt-1 text-xs text-muted">
        New uploads are mirrored as they land (an outage never blocks an upload); "Sync all" copies
        everything already stored — disk, built-in, or external — into the replica bucket.
      </p>
      {#if cfg.replica.enabled}
        <div class="mt-2 grid gap-2 sm:grid-cols-2">
          <TargetFields t={cfg.replica} secret={replicaSecret} onChange={setReplica} onSecret={(v) => (replicaSecret = v)} />
          <div class="flex items-center gap-2 sm:col-span-2">
            <Button size="sm" variant="outline" onclick={() => void act('test-replica')} disabled={busy || !cfg.replica.hasSecret}>
              Test replica
            </Button>
            <Button size="sm" variant="outline" onclick={() => void act('sync')} disabled={busy || syncing || !cfg.replica.hasSecret || totalBlobs === 0}>
              Sync all to replica
            </Button>
          </div>
        </div>
      {/if}
    </div>

    <div class="mt-3 flex items-center gap-2">
      <Button size="sm" onclick={() => void save()} disabled={busy}>
        Save
      </Button>
      {#if inBucket}
        <Button size="sm" variant="outline" onclick={() => void act('test')} disabled={busy || (external && !cfg.hasSecret)}>
          Test connection
        </Button>
      {/if}
      {#if note}<span class={`text-xs ${note.ok ? 'text-muted' : 'text-danger'}`}>{note.text}</span>{/if}
    </div>
  {/if}
</Panel>
