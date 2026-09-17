import { getBlob } from '@/lib/fetch-json'

/** Fetch a same-origin file and save it through an `<a download>` click.
 *  `target="_blank"` on `/api/uploads/…` opens a browser tab (or navigates the
 *  desktop webview away); this keeps the bytes inside our chrome. */
export async function downloadFile(url: string, filename: string): Promise<void> {
  const blob = await getBlob(url)
  const objectUrl = URL.createObjectURL(blob)
  try {
    const a = document.createElement('a')
    a.href = objectUrl
    a.download = sanitizeFilename(filename)
    a.rel = 'noreferrer'
    document.body.appendChild(a)
    a.click()
    a.remove()
  } finally {
    URL.revokeObjectURL(objectUrl)
  }
}

export function sanitizeFilename(name: string): string {
  const trimmed = name.trim()
  const cleaned = trimmed.replace(/[/\\?%*:|"<>]/g, '_')
  return cleaned || 'download'
}
