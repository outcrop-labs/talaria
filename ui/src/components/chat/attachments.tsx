import { useEffect, useRef, useState } from 'react'
import { Paperclip, X, FileText, Loader2, BookOpen, Gem, Upload, Search } from 'lucide-react'
import { Link } from '@tanstack/react-router'
import { Input } from '@/components/ui/input'
import { searchKb } from '@/lib/kb'
import { attachmentUrl, humanSize, isImage, uploadFile, type Attachment } from '@/lib/attachments'

// The attach affordance — a context menu, not a bare file browser: attach
// KNOWLEDGE docs, ARTIFACTS, or uploaded files. Knowledge/artifact picks
// become reference chips (refType set); the server carries their content to
// the model and persists the chip on the message.
export function AttachButton({ onAttach, disabled }: { onAttach: (a: Attachment) => void; disabled?: boolean }) {
  const fileRef = useRef<HTMLInputElement>(null)
  const wrapRef = useRef<HTMLDivElement>(null)
  const [busy, setBusy] = useState(false)
  const [open, setOpen] = useState(false)
  const [pick, setPick] = useState<'kb-doc' | 'artifact' | null>(null)

  useEffect(() => {
    if (!open) return
    const onDoc = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) {
        setOpen(false)
        setPick(null)
      }
    }
    document.addEventListener('mousedown', onDoc)
    return () => document.removeEventListener('mousedown', onDoc)
  }, [open])

  const pickFiles = async (files: FileList | null) => {
    if (!files?.length) return
    setBusy(true)
    try {
      for (const f of Array.from(files)) {
        const r = await uploadFile(f)
        if ('id' in r) onAttach(r)
      }
    } finally {
      setBusy(false)
      if (fileRef.current) fileRef.current.value = ''
    }
  }

  const item = 'flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-xs text-fg transition-colors hover:bg-sidebar'

  return (
    <div ref={wrapRef} className="relative self-end mb-1">
      <input ref={fileRef} type="file" multiple hidden onChange={(e) => void pickFiles(e.target.files)} />
      <button
        type="button"
        title="Attach knowledge, artifacts, or files"
        disabled={disabled || busy}
        onClick={() => {
          setOpen((v) => !v)
          setPick(null)
        }}
        className="grid h-9 w-9 shrink-0 place-items-center rounded-lg text-muted transition-colors hover:bg-card hover:text-fg disabled:opacity-40"
      >
        {busy ? <Loader2 size={16} className="animate-spin" /> : <Paperclip size={16} />}
      </button>
      {open && (
        <div className="absolute bottom-full left-0 z-30 mb-1 w-64 rounded-xl border border-line bg-card p-1 shadow-lg">
          {pick ? (
            <RefPicker
              kind={pick}
              onPick={(a) => {
                onAttach(a)
                setOpen(false)
                setPick(null)
              }}
            />
          ) : (
            <>
              <button type="button" className={item} onClick={() => setPick('kb-doc')}>
                <BookOpen size={13} className="text-muted" /> Knowledge doc
              </button>
              <button type="button" className={item} onClick={() => setPick('artifact')}>
                <Gem size={13} className="text-muted" /> Artifact
              </button>
              <button
                type="button"
                className={item}
                onClick={() => {
                  setOpen(false)
                  fileRef.current?.click()
                }}
              >
                <Upload size={13} className="text-muted" /> Upload file
              </button>
            </>
          )}
        </div>
      )}
    </div>
  )
}

/** Search-and-pick for knowledge docs / artifacts inside the attach menu. */
function RefPicker({ kind, onPick }: { kind: 'kb-doc' | 'artifact'; onPick: (a: Attachment) => void }) {
  const [q, setQ] = useState('')
  const [results, setResults] = useState<Array<{ id: string; title: string }>>([])
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    let cancelled = false
    const t = setTimeout(async () => {
      setLoading(true)
      try {
        if (kind === 'kb-doc') {
          const hits = q.trim() ? await searchKb(q) : []
          if (!cancelled) setResults(hits.slice(0, 8).map((h) => ({ id: h.id, title: h.title || 'Untitled' })))
        } else {
          const r = await fetch('/api/artifacts', { credentials: 'same-origin' })
          const all = ((await r.json()) as { artifacts?: Array<{ id: string; title: string }> }).artifacts ?? []
          const needle = q.trim().toLowerCase()
          if (!cancelled) {
            setResults(
              all
                .filter((a) => !needle || a.title.toLowerCase().includes(needle))
                .slice(0, 8)
                .map((a) => ({ id: a.id, title: a.title || 'Untitled' })),
            )
          }
        }
      } finally {
        if (!cancelled) setLoading(false)
      }
    }, 200)
    return () => {
      cancelled = true
      clearTimeout(t)
    }
  }, [q, kind])

  return (
    <div className="p-1">
      <div className="mb-1 flex items-center gap-1.5 rounded-lg border border-line-subtle px-2">
        <Search size={12} className="shrink-0 text-muted" />
        <Input
          autoFocus
          size="sm"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder={kind === 'kb-doc' ? 'Search knowledge' : 'Search artifacts'}
          className="border-0 bg-transparent px-0 focus:border-0"
        />
      </div>
      <div className="max-h-48 overflow-y-auto">
        {loading && <div className="px-2 py-1.5 text-xs text-muted">Searching</div>}
        {!loading && results.length === 0 && (
          <div className="px-2 py-1.5 text-xs text-muted">{kind === 'kb-doc' && !q.trim() ? 'Type to search docs' : 'No matches'}</div>
        )}
        {results.map((r) => (
          <button
            key={r.id}
            type="button"
            onClick={() => onPick({ id: r.id, filename: r.title, mime: `ref/${kind}`, size: 0, refType: kind })}
            className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-xs text-fg transition-colors hover:bg-sidebar"
          >
            {kind === 'kb-doc' ? <BookOpen size={13} className="shrink-0 text-muted" /> : <Gem size={13} className="shrink-0 text-muted" />}
            <span className="min-w-0 flex-1 truncate">{r.title}</span>
          </button>
        ))}
      </div>
    </div>
  )
}

const refIcon = (refType: string) =>
  refType === 'kb-doc' ? <BookOpen size={14} className="shrink-0 text-muted" /> : <Gem size={14} className="shrink-0 text-muted" />

// The pending-attachments strip above the composer (with remove buttons).
export function PendingAttachments({ items, onRemove }: { items: Attachment[]; onRemove: (id: string) => void }) {
  if (!items.length) return null
  return (
    <div className="flex flex-wrap gap-2 px-2 pb-2">
      {items.map((a) => (
        <div key={a.id} className="flex items-center gap-2 rounded-lg border border-line-subtle bg-card/50 px-2 py-1 text-xs">
          {a.refType ? (
            refIcon(a.refType)
          ) : isImage(a.mime) ? (
            <img src={attachmentUrl(a.id)} alt={a.filename} className="h-6 w-6 rounded object-cover" />
          ) : (
            <FileText size={14} className="text-muted" />
          )}
          <span className="max-w-32 truncate text-fg">{a.filename}</span>
          <button type="button" onClick={() => onRemove(a.id)} className="text-muted hover:text-[color:var(--theme-danger)]">
            <X size={13} />
          </button>
        </div>
      ))}
    </div>
  )
}

// How attachments render inside a message: images inline (click to open),
// files as download chips, knowledge/artifact refs as link chips.
export function MessageAttachments({ items }: { items: Attachment[] }) {
  if (!items?.length) return null
  const refs = items.filter((a) => a.refType)
  const images = items.filter((a) => !a.refType && isImage(a.mime))
  const files = items.filter((a) => !a.refType && !isImage(a.mime))
  return (
    <div className="mt-2 space-y-2">
      {images.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {images.map((a) => (
            <a key={a.id} href={attachmentUrl(a.id)} target="_blank" rel="noreferrer">
              <img src={attachmentUrl(a.id)} alt={a.filename} className="max-h-48 rounded-xl border border-line-subtle object-cover" />
            </a>
          ))}
        </div>
      )}
      {refs.map((a) => (
        <Link
          key={a.id}
          to={a.refType === 'kb-doc' ? '/knowledge' : '/artifacts'}
          className="inline-flex items-center gap-2 rounded-lg border border-line-subtle bg-card/50 px-2.5 py-1.5 text-xs text-fg transition-colors hover:border-accent"
          title={a.refType === 'kb-doc' ? 'Attached knowledge doc' : 'Attached artifact'}
        >
          {refIcon(a.refType!)}
          <span className="max-w-48 truncate">{a.filename}</span>
        </Link>
      ))}
      {files.map((a) => (
        <a
          key={a.id}
          href={attachmentUrl(a.id)}
          target="_blank"
          rel="noreferrer"
          className="inline-flex items-center gap-2 rounded-lg border border-line-subtle bg-card/50 px-2.5 py-1.5 text-xs text-fg transition-colors hover:border-accent"
        >
          <FileText size={14} className="text-muted" />
          <span className="max-w-48 truncate">{a.filename}</span>
          <span className="text-muted">{humanSize(a.size)}</span>
        </a>
      ))}
    </div>
  )
}
