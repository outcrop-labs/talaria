export interface Attachment {
  id: string
  filename: string
  mime: string
  size: number
}

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
