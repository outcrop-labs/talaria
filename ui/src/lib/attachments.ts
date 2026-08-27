import { errorMessage, postJson } from '@/lib/fetch-json'

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
  // Resolve-only envelope on purpose: all three callers (RichEditor,
  // AttachButton, InboxChatPanel) read `.error` off the result instead of
  // catching. The door's rejection is folded into that shape.
  return postJson<Attachment>('/api/uploads', form).catch((e: unknown) => ({ error: errorMessage(e) }))
}

export const humanSize = (bytes: number) =>
  bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${Math.round(bytes / 1024)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`
