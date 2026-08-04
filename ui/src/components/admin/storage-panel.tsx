import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Panel } from '@/components/ui/panel'
import { GeneratingBars } from '@/components/ui/generating'
import { SectionHeader } from '@/components/ui/section-header'
import { confirm } from '@/components/ui/confirm'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { QueryError } from '@/components/ui/query-state'
import { Skeleton } from '@/components/ui/skeleton'
import { getJson } from '@/lib/fetch-json'

interface TargetConfig {
  endpoint: string
  region: string
  bucket: string
  accessKeyId: string
  pathStyle: boolean
  prefix: string
  hasSecret: boolean
}
interface JobStatus {
  running: boolean
  moved: number
  failed: number
  total: number
  error?: string
}
interface StorageAdmin {
  config: TargetConfig & { mode: 'local' | 'internal' | 's3'; replica: TargetConfig & { enabled: boolean } }
  stats: { local: number; s3: number; internal: number; localBytes: number }
  migrate: JobStatus | null
  sync: JobStatus | null
  internal: { endpoint: string; bucket: string }
}

const useStorageAdmin = () =>
  useQuery({
    queryKey: ['storage-admin'],
    queryFn: (): Promise<StorageAdmin> => getJson<StorageAdmin>('/api/admin/storage'),
    refetchInterval: (q) => (q.state.data?.migrate?.running || q.state.data?.sync?.running ? 3_000 : false),
  })

const fmtBytes = (n: number) => (n >= 1 << 30 ? `${(n / (1 << 30)).toFixed(1)} GB` : n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.ceil(n / 1024)} KB`)

// Bucket credential fields, shared by the external target and the replica.
function TargetFields({ t, secret, onChange, onSecret }: { t: TargetConfig; secret: string; onChange: (patch: Partial<TargetConfig>) => void; onSecret: (v: string) => void }) {
  return (
    <>
      <label className="text-xs text-muted">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Endpoint</span>
        <Input value={t.endpoint} onChange={(e) => onChange({ endpoint: e.target.value })} placeholder="https://s3.us-west-004.backblazeb2.com" className="mt-1 w-full" />
      </label>
      <label className="text-xs text-muted">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Bucket</span>
        <Input value={t.bucket} onChange={(e) => onChange({ bucket: e.target.value })} placeholder="talaria-uploads" className="mt-1 w-full" />
      </label>
      <label className="text-xs text-muted">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Region</span> <span className="opacity-70">(blank = derived from endpoint)</span>
        <Input value={t.region} onChange={(e) => onChange({ region: e.target.value })} placeholder="auto" className="mt-1 w-full" />
      </label>
      <label className="text-xs text-muted">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Key prefix</span> <span className="opacity-70">(optional, ends with /)</span>
        <Input value={t.prefix} onChange={(e) => onChange({ prefix: e.target.value })} placeholder="talaria/" className="mt-1 w-full" />
      </label>
      <label className="text-xs text-muted">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Access key ID</span>
        <Input value={t.accessKeyId} onChange={(e) => onChange({ accessKeyId: e.target.value })} className="mt-1 w-full" />
      </label>
      <label className="text-xs text-muted">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Secret access key</span>
        <Input type="password" value={secret} onChange={(e) => onSecret(e.target.value)} placeholder={t.hasSecret ? '•••••••• (saved)' : ''} className="mt-1 w-full" />
      </label>
      <label className="flex items-center gap-2 text-xs text-muted sm:col-span-2">
        <input type="checkbox" checked={t.pathStyle} onChange={(e) => onChange({ pathStyle: e.target.checked })} className="accent-accent" />
        Path-style requests <span className="opacity-70">(works everywhere; uncheck only for virtual-host buckets)</span>
      </label>
    </>
  )
}

// Where upload blobs live: local disk, the bundled MinIO container ("built-in
// bucket"), or any external S3-compatible service — plus an optional replica
// that mirrors every blob to a second provider.
export function StoragePanel() {
  const qc = useQueryClient()
  const query = useStorageAdmin()
  const { data } = query
  const [form, setForm] = useState<StorageAdmin['config'] | null>(null)
  const [secret, setSecret] = useState('')
  const [replicaSecret, setReplicaSecret] = useState('')
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null)

  useEffect(() => {
    if (data && !form) setForm(data.config)
  }, [data, form])
  // A failed read used to leave `data` undefined for ever, so this panel
  // shimmered its skeleton at a request that had already died — a loading
  // state that never ends is just a slower blank screen.
  if (!data && query.isError)
    return (
      <Panel>
        <QueryError error={query.error} title="Could not load storage settings" onRetry={() => void query.refetch()} />
      </Panel>
    )
  if (!data || !form)
    // Panel-shaped skeleton (title, stat strip, config grid, button row) so
    // the Storage tab never renders blank and then materializes.
    return (
      <Panel>
        <Skeleton className="mb-4 h-4 w-20 rounded-full" />
        <div className="mb-3 flex flex-wrap items-center gap-3 rounded-md border border-line p-3">
          <Skeleton className="h-3 w-32 rounded-full" />
          <Skeleton className="h-3 w-36 rounded-full" delay={0.12} />
          <Skeleton className="h-3 w-32 rounded-full" delay={0.24} />
        </div>
        <div className="grid gap-2 sm:grid-cols-2">
          {Array.from({ length: 6 }, (_, i) => (
            <div key={i}>
              <Skeleton className="mb-1.5 h-2.5 w-24 rounded-full" delay={i * 0.08} />
              <Skeleton className="h-9 w-full" delay={i * 0.08} />
            </div>
          ))}
        </div>
        <div className="mt-3 flex items-center gap-2">
          <Skeleton className="h-7 w-16" />
          <Skeleton className="h-7 w-32" delay={0.12} />
        </div>
      </Panel>
    )

  const set = (patch: Partial<StorageAdmin['config']>) => setForm({ ...form, ...patch })
  const setReplica = (patch: Partial<StorageAdmin['config']['replica']>) => setForm({ ...form, replica: { ...form.replica, ...patch } })

  const save = async () => {
    setBusy(true)
    setNote(null)
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
        setNote({ ok: false, text: j.error ?? 'save failed' })
        return
      }
      setForm(j.config)
      setSecret('')
      setReplicaSecret('')
      setNote({ ok: true, text: 'saved' })
      await qc.invalidateQueries({ queryKey: ['storage-admin'] })
    } finally {
      setBusy(false)
    }
  }

  const act = async (action: 'test' | 'test-replica' | 'migrate' | 'sync') => {
    setBusy(true)
    if (action === 'test' || action === 'test-replica') setNote(null)
    try {
      const r = await fetch('/api/admin/storage', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action }) })
      const j = (await r.json()) as { ok?: boolean; detail?: string; error?: string }
      if (action === 'test' || action === 'test-replica') setNote({ ok: !!j.ok, text: j.detail ?? j.error ?? '' })
      else if (j.error) setNote({ ok: false, text: j.error })
      await qc.invalidateQueries({ queryKey: ['storage-admin'] })
    } finally {
      setBusy(false)
    }
  }

  const migrate = async () => {
    const ok = await confirm({
      title: 'Move local files to the bucket?',
      message: `Copies ${data.stats.local} file${data.stats.local === 1 ? '' : 's'} (${fmtBytes(data.stats.localBytes)}) into the active bucket and repoints each record. Local copies are left on disk.`,
      confirmLabel: 'Move files',
    })
    if (ok) await act('migrate')
  }

  const inBucket = form.mode !== 'local'
  const external = form.mode === 's3'
  const migrating = data.migrate?.running
  const syncing = data.sync?.running
  const totalBlobs = data.stats.local + data.stats.s3 + data.stats.internal

  return (
    <Panel>
      <SectionHeader
        className="mb-4"
        title="Storage"
        info="Where uploaded files live. Local disk keeps everything on this machine; the built-in bucket is Talaria's own bundled object store (no cloud account needed); external works with any S3-compatible service — AWS S3, Backblaze B2, Cloudflare R2, MinIO. Each file remembers where it was stored, so switching never breaks existing links. A replica mirrors every file to a second provider for redundancy."
      />

      <div className="mb-3 flex flex-wrap items-center gap-3 rounded-md border border-line p-3 text-xs text-muted">
        <span><span className="font-mono text-fg">{data.stats.local}</span> on disk (<span className="font-mono">{fmtBytes(data.stats.localBytes)}</span>)</span>
        <span><span className="font-mono text-fg">{data.stats.internal}</span> in the built-in bucket</span>
        <span><span className="font-mono text-fg">{data.stats.s3}</span> in external storage</span>
        {migrating && (
          <span className="flex items-center gap-1.5"><GeneratingBars bars={3} variant="breathe" step={0.2} /> moving <span className="font-mono">{data.migrate!.moved}/{data.migrate!.total}</span></span>
        )}
        {syncing && (
          <span className="flex items-center gap-1.5"><GeneratingBars bars={3} variant="breathe" step={0.2} /> syncing <span className="font-mono">{data.sync!.moved}/{data.sync!.total}</span> to replica</span>
        )}
        {!migrating && data.migrate?.failed ? <span className="text-danger">{data.migrate.failed} failed to move</span> : null}
        {!syncing && data.sync?.failed ? <span className="text-danger">{data.sync.failed} failed to sync</span> : null}
        {inBucket && data.stats.local > 0 && !migrating && (
          <Button size="sm" variant="outline" className="ml-auto" onClick={() => void migrate()} disabled={busy}>
            Move local files to bucket
          </Button>
        )}
      </div>

      <div className="grid gap-2 sm:grid-cols-2">
        <label className="text-xs text-muted">
          <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Mode</span>
          <Select value={form.mode} onChange={(e) => set({ mode: e.target.value as StorageAdmin['config']['mode'] })} className="mt-1 w-full">
            <option value="local">Local disk</option>
            <option value="internal">Built-in bucket (bundled MinIO)</option>
            <option value="s3">External (S3-compatible)</option>
          </Select>
        </label>
        {form.mode === 'internal' && (
          <div className="self-end pb-1 text-xs text-muted">
            <span className="font-mono text-[11px]">{data.internal.endpoint}</span> · bucket <span className="font-mono text-[11px] text-fg">{data.internal.bucket}</span>
            <span className="opacity-70"> — creds via TALARIA_S3_* env; bucket auto-created</span>
          </div>
        )}
        {external && <TargetFields t={form} secret={secret} onChange={set} onSecret={setSecret} />}
      </div>

      {/* Replica — mirror every blob to a second provider */}
      <div className="mt-4 rounded-md border border-line p-3">
        <label className="flex items-center gap-2 text-xs font-medium text-fg">
          <input type="checkbox" checked={form.replica.enabled} onChange={(e) => setReplica({ enabled: e.target.checked })} className="accent-accent" />
          Replicate to a second provider
        </label>
        <p className="mt-1 text-xs text-muted">
          New uploads are mirrored as they land (an outage never blocks an upload); "Sync all" copies
          everything already stored — disk, built-in, or external — into the replica bucket.
        </p>
        {form.replica.enabled && (
          <div className="mt-2 grid gap-2 sm:grid-cols-2">
            <TargetFields t={form.replica} secret={replicaSecret} onChange={setReplica} onSecret={setReplicaSecret} />
            <div className="flex items-center gap-2 sm:col-span-2">
              <Button size="sm" variant="outline" onClick={() => void act('test-replica')} disabled={busy || !form.replica.hasSecret}>
                Test replica
              </Button>
              <Button size="sm" variant="outline" onClick={() => void act('sync')} disabled={busy || syncing || !form.replica.hasSecret || totalBlobs === 0}>
                Sync all to replica
              </Button>
            </div>
          </div>
        )}
      </div>

      <div className="mt-3 flex items-center gap-2">
        <Button size="sm" onClick={() => void save()} disabled={busy}>
          Save
        </Button>
        {inBucket && (
          <Button size="sm" variant="outline" onClick={() => void act('test')} disabled={busy || (external && !form.hasSecret)}>
            Test connection
          </Button>
        )}
        {note && <span className={`text-xs ${note.ok ? 'text-muted' : 'text-danger'}`}>{note.text}</span>}
      </div>
    </Panel>
  )
}
