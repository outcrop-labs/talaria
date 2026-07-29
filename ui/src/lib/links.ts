/** Copy an app path as a full URL — the standard "Copy link" action. */
export function copyAppLink(path: string): void {
  void navigator.clipboard.writeText(`${window.location.origin}${path}`)
}
