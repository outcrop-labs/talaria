// An app throw must not become a host throw. Server dispatch, MCP, and the
// builder all run through isolateApp so a crashing app is a structured
// failure the UI can show, not an unhandled rejection that takes the
// process with it. process.exit / infinite loops are still same-process
// (apps run fully trusted); this is the exception boundary.

export type Isolated<T> = { ok: true; value: T } | { ok: false; error: string }

export function errorMessageOf(e: unknown): string {
  if (e instanceof Error) return e.message || e.name
  if (typeof e === 'string') return e
  try {
    return JSON.stringify(e)
  } catch {
    return String(e)
  }
}

export function crashText(e: unknown): { message: string; stack: string } {
  if (e instanceof Error) {
    return { message: e.message || e.name, stack: e.stack ?? '' }
  }
  return { message: errorMessageOf(e), stack: '' }
}

export async function isolateApp<T>(slug: string, label: string, fn: () => Promise<T>): Promise<Isolated<T>> {
  try {
    return { ok: true, value: await fn() }
  } catch (e) {
    console.error(`[apps:${slug}] ${label}`, e)
    return { ok: false, error: errorMessageOf(e) }
  }
}
