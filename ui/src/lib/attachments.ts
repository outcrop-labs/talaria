export interface Attachment {
  id: string
  filename: string
  mime: string
  size: number
  /** Set for knowledge/artifact reference chips (not uploads). */
  refType?: 'kb-doc' | 'artifact'
}

/** Split a pending list into upload ids + reference descriptors for send. */
export const splitAttachments = (items: Attachment[]) => ({
  attachmentIds: items.filter((a) => !a.refType).map((a) => a.id),
  refs: items.filter((a) => a.refType).map((a) => ({ type: a.refType!, id: a.id })),
})

export const isImage = (mime: string) => /^image\//.test(mime)
export const attachmentUrl = (id: string) => `/api/uploads/${id}`

export async function uploadFile(file: File): Promise<Attachment | { error: string }> {
  const form = new FormData()
  form.append('file', file)
  const r = await fetch('/api/uploads', { method: 'POST', body: form }).catch(() => null)
  if (!r?.ok) return { error: (await r?.json?.().catch(() => null))?.error ?? 'upload failed' }
  return r.json()
}

export const humanSize = (bytes: number) =>
  bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`
