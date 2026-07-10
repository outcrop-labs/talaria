import { useRef, useState } from 'react'
import { Paperclip, X, FileText, Loader2 } from 'lucide-react'
import { attachmentUrl, humanSize, isImage, uploadFile, type Attachment } from '@/lib/attachments'

// Attach button + hidden file input. Uploads immediately and hands back the
// stored attachment; the composer holds the pending list.
export function AttachButton({ onAttach, disabled }: { onAttach: (a: Attachment) => void; disabled?: boolean }) {
  const ref = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const pick = async (files: FileList | null) => {
    if (!files?.length) return
    setBusy(true)
    try {
      for (const f of Array.from(files)) {
        const r = await uploadFile(f)
        if ('id' in r) onAttach(r)
      }
    } finally {
      setBusy(false)
      if (ref.current) ref.current.value = ''
    }
  }
  return (
    <>
      <input ref={ref} type="file" multiple hidden onChange={(e) => void pick(e.target.files)} />
      <button
        type="button"
        title="Attach files"
        disabled={disabled || busy}
        onClick={() => ref.current?.click()}
        className="grid h-9 w-9 shrink-0 place-items-center self-end mb-1 rounded-lg text-muted transition-colors hover:bg-card hover:text-fg disabled:opacity-40"
      >
        {busy ? <Loader2 size={16} className="animate-spin" /> : <Paperclip size={16} />}
      </button>
    </>
  )
}

// The pending-attachments strip above the composer (with remove buttons).
export function PendingAttachments({ items, onRemove }: { items: Attachment[]; onRemove: (id: string) => void }) {
  if (!items.length) return null
  return (
    <div className="flex flex-wrap gap-2 px-2 pb-2">
      {items.map((a) => (
        <div key={a.id} className="flex items-center gap-2 rounded-lg border border-line-subtle bg-card/50 px-2 py-1 text-xs">
          {isImage(a.mime) ? (
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

// How attachments render inside a message: images inline (click to open), other
// files as download chips.
export function MessageAttachments({ items }: { items: Attachment[] }) {
  if (!items?.length) return null
  const images = items.filter((a) => isImage(a.mime))
  const files = items.filter((a) => !isImage(a.mime))
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
