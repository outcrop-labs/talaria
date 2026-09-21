// The one in-app file viewer. Chat chips, board attachments, artifact
// downloads — anything that used to `target="_blank"` a `/api/uploads/…` URL
// — open this instead. Preview when we can, an honest "cannot preview" when
// we cannot, Download either way. Desktop webviews have no browser chrome
// to "Save as" from a new tab.

export interface FileViewerFile {
  url: string
  title: string
  contentType: string | null
  sizeBytes?: number | null
}

let current = $state<FileViewerFile | null>(null)

export function fileViewer(): FileViewerFile | null {
  return current
}

export function openFileViewer(file: FileViewerFile): void {
  current = file
}

export function closeFileViewer(): void {
  current = null
}
