export function shouldAttachInboxDecision(pathname: string, tab: string | undefined): boolean {
  return pathname === '/' && (tab === undefined || tab === 'inbox')
}

/** Where the person is standing when they talk to the assistant. */
export interface AssistantSurface {
  /** Stable id sent to the server, which owns the prose. Never free text: the
   *  command endpoint is reachable by any signed-in client, and an id the
   *  server has to recognise cannot be used to write the prompt from outside. */
  id: string
  /** What the panel shows under the assistant's name. */
  label: string
}

// Longest prefix wins, so '/boards/:id/:taskId' resolves through '/boards'.
// Ordered most specific first; `/` is the fallthrough.
const SURFACES: Array<{ prefix: string; id: string; label: string }> = [
  { prefix: '/chat', id: 'chat', label: 'Chat' },
  { prefix: '/comms', id: 'comms', label: 'Comms' },
  { prefix: '/channels', id: 'comms', label: 'Comms' },
  { prefix: '/inbox', id: 'inbox', label: 'Inbox' },
  { prefix: '/boards', id: 'boards', label: 'Boards' },
  { prefix: '/plan', id: 'plan', label: 'Plan' },
  { prefix: '/research', id: 'research', label: 'Research' },
  { prefix: '/knowledge', id: 'knowledge', label: 'Knowledge' },
  { prefix: '/artifacts', id: 'artifacts', label: 'Files' },
  { prefix: '/agents', id: 'agents', label: 'Agents' },
  { prefix: '/fleet', id: 'fleet', label: 'Fleet' },
  { prefix: '/studio', id: 'studio', label: 'Agent Studio' },
  { prefix: '/templates', id: 'templates', label: 'Templates' },
  { prefix: '/models', id: 'models', label: 'Models' },
  { prefix: '/mcp', id: 'mcp', label: 'MCP' },
  { prefix: '/observability', id: 'observability', label: 'Observability' },
  { prefix: '/apps', id: 'apps', label: 'Apps' },
  { prefix: '/x', id: 'apps', label: 'Apps' },
  { prefix: '/settings', id: 'settings', label: 'Settings' },
  { prefix: '/admin', id: 'admin', label: 'Admin' },
]

/** Every id this module can produce. The server holds the matching prose, and
 *  a test asserts the two sets line up — a surface added here without a brief
 *  there degrades silently to "no context", which is exactly the bug this
 *  whole mechanism exists to fix. */
export const ASSISTANT_SURFACE_IDS: string[] = [...new Set(['inbox', 'home', ...SURFACES.map((s) => s.id)])]

/** The view the assistant panel is currently floating over.
 *
 *  The panel is launched from the nav rail on EVERY view, but its conversation
 *  was built for one — so on Boards it opened as a "general Inbox conversation"
 *  and answered as if you had been reading your queue. The surface travels with
 *  the request so the answer is about the page you are on. */
export function assistantSurface(pathname: string, tab: string | undefined): AssistantSurface {
  // Home is a tabbed view and one of its tabs IS the focus queue.
  if (pathname === '/') {
    return shouldAttachInboxDecision(pathname, tab)
      ? { id: 'inbox', label: 'Inbox' }
      : { id: 'home', label: 'Home' }
  }
  const match = SURFACES.filter((s) => pathname === s.prefix || pathname.startsWith(`${s.prefix}/`)).sort(
    (a, b) => b.prefix.length - a.prefix.length,
  )[0]
  return match ? { id: match.id, label: match.label } : { id: 'home', label: 'Home' }
}
