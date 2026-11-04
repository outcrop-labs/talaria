import { createRouter } from 'sv-router'
import { bindViewTitlePath } from './lib/view-title.svelte'
import { resolveHref } from '@/lib/route-tabs'
import { routes } from './route-table'

// Server-path anchors vs. sv-router's global click interceptor.
//
// Router.svelte listens for (bubble-phase) clicks on window and rewrites ANY
// same-origin <a> into a client-side navigation — including plain anchors to
// server routes (the Google login buttons, every Google "Connect" link). Those
// paths have no client route, so the catch-all rendered NotFound at e.g.
// /api/integrations/google/org/connect and the request never reached the
// server. Capture-phase runs before that handler; stopping propagation for
// same-origin /api (and /.well-known) anchors — without preventDefault — lets
// the browser navigate them for real. Same path predicate as the dev
// middleware and server/app.ts.
if (typeof document !== 'undefined') {
  document.addEventListener(
    'click',
    (event) => {
      if (event.button !== 0 || event.defaultPrevented) return
      const anchor = (event.target as HTMLElement | null)?.closest?.('a')
      if (!anchor?.hasAttribute('href')) return
      const url = new URL(anchor.href, location.href)
      if (url.origin !== location.origin) return
      if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/.well-known/')) {
        event.stopPropagation()
      }
    },
    { capture: true },
  )
}

// The route map lives in route-table.ts, where a node test can run the
// matcher over it; this module is the router built from it.
export const { p, navigate, isActive, preload, route } = createRouter(routes)

// View-title claims are keyed by this router's pathname. The binding runs
// this way round so that module stays loadable outside a browser (see
// lib/view-title.svelte.ts).
bindViewTitlePath(() => route.pathname)

/** Navigate to a server-supplied href (alert / notification / activity links)
 *  or an app's own (appNavigate). The typed `navigate` wants a route-path
 *  literal; these arrive as plain strings, often with a query string
 *  (`/artifacts?a=<id>`), which `navigate` would match as part of the path —
 *  so the query and hash go in its options. One widening cast here instead
 *  of an unsound one at every call site.
 *
 *  THE HREF IS RESOLVED FIRST, as the browser resolves it (resolveHref).
 *  sv-router matches the string it is handed but writes the URL-normalized
 *  path to the address bar, and the address is what the route gate checks —
 *  so `/x/crm/manage/../items` mounted the manage surface at the address
 *  /x/crm/items, which the gate judged as the work view. Resolved, the router
 *  and the gate read one path. An href to another origin is refused — the
 *  promise rejects and nothing moves. */
export const navigateHref = async (href: string, options?: { replace?: boolean }): Promise<unknown> => {
  const { pathname, search, hash } = resolveHref(href, location.href)
  type Untyped = (path: string, options?: { replace?: boolean; search?: string; hash?: string }) => Promise<unknown>
  return (navigate as unknown as Untyped)(pathname, {
    ...options,
    ...(search ? { search } : {}),
    ...(hash ? { hash } : {}),
  })
}
