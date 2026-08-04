import { useState } from 'react'
import { Link } from '@tanstack/react-router'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Modal } from '@/components/ui/modal'
import { Select } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { listQuery } from '@/components/ui/query-state'
import { useFolders } from '@/lib/artifacts'

// An agent-produced image in chat, with a hover affordance to keep it: "Save
// to Artifacts" copies the file out of the agent's container into a durable
// FILE artifact, optionally straight into a folder (science / memes / etc).
export function AgentMediaImage({ src, alt }: { src: string; alt: string }) {
  const [open, setOpen] = useState(false)
  return (
    <span className="group relative my-2 inline-block">
      <img src={src} alt={alt} className="max-h-96 rounded-lg border border-line" />
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="absolute right-2 top-2 rounded-lg border border-line-subtle bg-card px-2 py-1 text-xs text-muted opacity-0 transition-opacity hover:text-fg group-hover:opacity-100"
      >
        Save to Artifacts
      </button>
      {open && <SaveDialog src={src} onClose={() => setOpen(false)} />}
    </span>
  )
}

function SaveDialog({ src, onClose }: { src: string; onClose: () => void }) {
  // A failed folder read used to leave "No folder (root)" as the only option,
  // so the image was filed at the root and the owner had no idea their folders
  // were simply unreadable at that moment.
  const {
    rows: folders,
    notice: foldersNotice,
    pending: foldersLoading,
  } = listQuery(useFolders(), { title: 'Could not load your folders', variant: 'inline' })
  const url = new URL(src, window.location.origin)
  const path = url.searchParams.get('path') ?? ''
  const model = decodeURIComponent(url.pathname.split('/').pop() ?? '')
  const [title, setTitle] = useState(path.split('/').pop() ?? 'image')
  const [folderId, setFolderId] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [savedId, setSavedId] = useState<string | null>(null)

  const save = async () => {
    setBusy(true)
    setError(null)
    try {
      const r = await fetch(`/api/agent-media/${encodeURIComponent(model)}/save`, {
        method: 'POST',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ path, title: title.trim() || undefined, folderId: folderId || null }),
      })
      const j = (await r.json().catch(() => ({}))) as { artifact?: { id: string }; error?: string }
      if (!r.ok || !j.artifact) throw new Error(j.error ?? `save failed (${r.status})`)
      setSavedId(j.artifact.id)
    } catch (e) {
      setError((e as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal open onClose={onClose} title="Save to Artifacts">
      {savedId ? (
        <div className="space-y-3">
          <p className="text-sm text-fg">
            Saved. It's a durable file artifact now: versioned, shareable, folder-organized.
          </p>
          <div className="flex justify-end gap-2 border-t border-line-subtle pt-3">
            <Link to="/artifacts" className="text-sm text-accent hover:underline" onClick={onClose}>
              Open Artifacts ↗
            </Link>
            <Button size="sm" onClick={onClose}>
              Done
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-3">
          <div>
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Title</label>
            <Input size="sm" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} />
          </div>
          <div>
            <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Folder</label>
            {foldersLoading ? (
              <Skeleton className="h-9 w-full" />
            ) : (
              <Select size="sm" value={folderId} onChange={(e) => setFolderId(e.target.value)} className="w-full">
                <option value="">No folder (root)</option>
                {folders.map((f) => (
                  <option key={f.id} value={f.id}>
                    {f.name}
                  </option>
                ))}
              </Select>
            )}
            {foldersNotice}
          </div>
          {error && (
            <div className="text-xs" style={{ color: 'var(--theme-danger)' }}>
              {error}
            </div>
          )}
          <div className="flex justify-end gap-2 border-t border-line-subtle pt-3">
            <Button variant="ghost" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <Button size="sm" onClick={() => void save()} disabled={busy}>
              {busy ? 'Saving' : 'Save'}
            </Button>
          </div>
        </div>
      )}
    </Modal>
  )
}
