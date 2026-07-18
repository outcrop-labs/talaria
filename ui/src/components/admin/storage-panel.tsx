import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { Panel } from '@/components/ui/panel'
import { confirm } from '@/components/ui/confirm'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'

interface StorageAdmin {
  config: {
    mode: 'local' | 's3'
    endpoint: string
    region: string
    bucket: string
    accessKeyId: string
    pathStyle: boolean
    prefix: string
    hasSecret: boolean
  }
  stats: { local: number; s3: number; localBytes: number }
  migrate: { running: boolean; moved: number; failed: number; total: number; error?: string } | null
}

const useStorageAdmin = () =>
  useQuery({
    queryKey: ['storage-admin'],
    queryFn: async (): Promise<StorageAdmin> => {
      const r = await fetch('/api/admin/storage')
      if (!r.ok) throw new Error('failed')
      return r.json()
    },
    refetchInterval: (q) => (q.state.data?.migrate?.running ? 3_000 : false),
  })

const fmtBytes = (n: number) => (n >= 1 << 30 ? `${(n / (1 << 30)).toFixed(1)} GB` : n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.ceil(n / 1024)} KB`)

// Where upload blobs live: local disk (default) or any S3-compatible bucket —
// AWS S3, Backblaze B2, Cloudflare R2, MinIO. Endpoint-generic on purpose.
export function StoragePanel() {
  const qc = useQueryClient()
  const { data } = useStorageAdmin()
  const [form, setForm] = useState<StorageAdmin['config'] | null>(null)
  const [secret, setSecret] = useState('')
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null)

  useEffect(() => {
    if (data && !form) setForm(data.config)
  }, [data, form])
  if (!data || !form) return null

  const set = (patch: Partial<StorageAdmin['config']>) => setForm({ ...form, ...patch })

  const save = async () => {
    setBusy(true)
    setNote(null)
    try {
      const r = await fetch('/api/admin/storage', {
        method: 'PUT',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ ...form, secretAccessKey: secret || undefined }),
      })
      const j = (await r.json()) as { config?: StorageAdmin['config']; error?: string }
      if (!r.ok || !j.config) {
        setNote({ ok: false, text: j.error ?? 'save failed' })
        return
      }
      setForm(j.config)
      setSecret('')
      setNote({ ok: true, text: 'saved' })
      await qc.invalidateQueries({ queryKey: ['storage-admin'] })
    } finally {
      setBusy(false)
    }
  }

  const test = async () => {
    setBusy(true)
    setNote(null)
    try {
      const r = await fetch('/api/admin/storage', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'test' }) })
      const j = (await r.json()) as { ok: boolean; detail: string }
      setNote({ ok: j.ok, text: j.detail })
    } finally {
      setBusy(false)
    }
  }

  const migrate = async () => {
    const ok = await confirm({
      title: 'Move local files to the bucket?',
      message: `Copies ${data.stats.local} file${data.stats.local === 1 ? '' : 's'} (${fmtBytes(data.stats.localBytes)}) into the configured bucket and repoints each record. Local copies are left on disk.`,
      confirmLabel: 'Move files',
    })
    if (!ok) return
    await fetch('/api/admin/storage', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ action: 'migrate' }) })
    await qc.invalidateQueries({ queryKey: ['storage-admin'] })
  }

  const s3 = form.mode === 's3'
  const migrating = data.migrate?.running

  return (
    <Panel>
      <div className="mb-2 text-sm font-semibold text-fg">Storage</div>
      <p className="mb-3 text-xs text-muted">
        Where uploaded files live. <span className="text-fg">Local disk</span> keeps everything on this
        machine; <span className="text-fg">object storage</span> works with any S3-compatible service —
        AWS S3, Backblaze B2, Cloudflare R2, MinIO. Each file remembers where it was stored, so switching
        never breaks existing links.
      </p>

      <div className="mb-3 flex flex-wrap items-center gap-3 rounded-xl border border-line-subtle p-3 text-xs text-muted">
        <span><span className="text-fg">{data.stats.local}</span> on disk ({fmtBytes(data.stats.localBytes)})</span>
        <span><span className="text-fg">{data.stats.s3}</span> in object storage</span>
        {migrating && (
          <span className="flex items-center gap-1.5"><Loader2 size={12} className="animate-spin" /> moving {data.migrate!.moved}/{data.migrate!.total}</span>
        )}
        {!migrating && data.migrate?.failed ? <span className="text-[color:var(--theme-danger)]">{data.migrate.failed} failed to move</span> : null}
        {s3 && form.hasSecret && data.stats.local > 0 && !migrating && (
          <Button size="sm" variant="outline" className="ml-auto" onClick={() => void migrate()}>
            Move local files to bucket
          </Button>
        )}
      </div>

      <div className="grid gap-2 sm:grid-cols-2">
        <label className="text-xs text-muted">
          Mode
          <Select value={form.mode} onChange={(e) => set({ mode: e.target.value as 'local' | 's3' })} className="mt-1 w-full">
            <option value="local">Local disk</option>
            <option value="s3">Object storage (S3-compatible)</option>
          </Select>
        </label>
        {s3 && (
          <>
            <label className="text-xs text-muted">
              Endpoint
              <Input value={form.endpoint} onChange={(e) => set({ endpoint: e.target.value })} placeholder="https://s3.us-west-004.backblazeb2.com" className="mt-1 w-full" />
            </label>
            <label className="text-xs text-muted">
              Bucket
              <Input value={form.bucket} onChange={(e) => set({ bucket: e.target.value })} placeholder="talaria-uploads" className="mt-1 w-full" />
            </label>
            <label className="text-xs text-muted">
              Region <span className="opacity-70">(blank = derived from endpoint)</span>
              <Input value={form.region} onChange={(e) => set({ region: e.target.value })} placeholder="auto" className="mt-1 w-full" />
            </label>
            <label className="text-xs text-muted">
              Key prefix <span className="opacity-70">(optional, ends with /)</span>
              <Input value={form.prefix} onChange={(e) => set({ prefix: e.target.value })} placeholder="talaria/" className="mt-1 w-full" />
            </label>
            <label className="text-xs text-muted">
              Access key ID
              <Input value={form.accessKeyId} onChange={(e) => set({ accessKeyId: e.target.value })} className="mt-1 w-full" />
            </label>
            <label className="text-xs text-muted">
              Secret access key
              <Input type="password" value={secret} onChange={(e) => setSecret(e.target.value)} placeholder={form.hasSecret ? '•••••••• (saved)' : ''} className="mt-1 w-full" />
            </label>
            <label className="flex items-center gap-2 text-xs text-muted sm:col-span-2">
              <input type="checkbox" checked={form.pathStyle} onChange={(e) => set({ pathStyle: e.target.checked })} />
              Path-style requests <span className="opacity-70">(works everywhere; uncheck only for virtual-host buckets)</span>
            </label>
          </>
        )}
      </div>

      <div className="mt-3 flex items-center gap-2">
        <Button size="sm" onClick={() => void save()} disabled={busy}>
          Save
        </Button>
        {s3 && (
          <Button size="sm" variant="outline" onClick={() => void test()} disabled={busy || !form.hasSecret}>
            Test connection
          </Button>
        )}
        {note && <span className={`text-xs ${note.ok ? 'text-muted' : 'text-[color:var(--theme-danger)]'}`}>{note.text}</span>}
      </div>
    </Panel>
  )
}
