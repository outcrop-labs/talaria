// Error-classification helpers for the one error surface (ErrorFallback.svelte).
// Talaria shipped without any error net: a render throw — most often a lazy
// chunk whose hash no longer exists after a deploy — white-screened the whole
// cockpit with no message and no way back.

/** A chunk the server no longer has: the deploy moved out from under this tab. */
export function isStaleChunkError(error: unknown): boolean {
  const msg = error instanceof Error ? error.message : String(error ?? '')
  // "Unable to preload CSS for /assets/x-9f3c.css" is Vite's own preload helper
  // throwing for exactly this cause — the asset hash rotated under an open tab.
  // Same deploy, same fix; it must not fall through to the generic message.
  return /dynamically imported module|module script failed|Loading chunk|ChunkLoadError|Failed to fetch dynamically|unable to preload css/i.test(msg)
}

export function messageOf(error: unknown): string {
  const msg = error instanceof Error ? error.message : typeof error === 'string' ? error : ''
  return msg.length > 220 ? `${msg.slice(0, 220)}…` : msg
}
