import { createRouter } from 'sv-router'
import NotFound from './routes/NotFound.svelte'

// The whole route map, explicit (sv-router code-based tree mode). Lazy
// `() => import(...)` per view keeps code splitting; the app shell
// (AppLayout) is the layout for everything behind the session gate — and it
// is lazy TOO (the old _app.tsx was a lazy file route for the same reason):
// eager, it hoists the whole shell graph — InboxFocusShell → InboxChatPanel →
// RichEditor → tiptap/prosemirror — into the entry chunk that /login and the
// public share pages must also download.
//
// Old TanStack file routes → this tree:
//   routes/login.tsx            → '/login'
//   routes/join.tsx             → '/join'
//   routes/a.$slug.tsx          → '/a/:slug'      (public artifact share)
//   routes/kb.$slug.tsx         → '/kb/:slug'     (public doc share)
//   routes/kb.space.$slug.tsx   → '/kb/space/:slug'
//   routes/_app.tsx             → layout: AppLayout
//   routes/_app/<view>.tsx      → '/<view>'
//   routes/_app/x.$app.*.tsx    → '/x/:app'(+/manage)
//   routes/_app/boards/*        → '/boards', '/boards/:boardId(/:taskId)'
export const { p, navigate, isActive, preload, route } = createRouter({
  '/login': () => import('./routes/Login.svelte'),
  '/join': () => import('./routes/Join.svelte'),
  '/a': {
    '/:slug': () => import('./routes/ArtifactPublic.svelte'),
  },
  '/kb': {
    '/:slug': () => import('./routes/KbDocPublic.svelte'),
    '/space': {
      '/:slug': () => import('./routes/KbSpacePublic.svelte'),
    },
  },
  '/': {
    '/': () => import('./routes/app/Home.svelte'),
    '/chat': () => import('./routes/app/Chat.svelte'),
    '/comms': () => import('./routes/app/Comms.svelte'),
    '/channels': () => import('./routes/app/Channels.svelte'),
    '/inbox': () => import('./routes/app/Inbox.svelte'),
    '/boards': {
      '/': () => import('./routes/app/boards/BoardsIndex.svelte'),
      '/:boardId': {
        '/': () => import('./routes/app/boards/Board.svelte'),
        '/:taskId': () => import('./routes/app/boards/Task.svelte'),
      },
    },
    '/plan': () => import('./routes/app/Plan.svelte'),
    '/research': () => import('./routes/app/Research.svelte'),
    '/knowledge': () => import('./routes/app/Knowledge.svelte'),
    '/artifacts': () => import('./routes/app/Artifacts.svelte'),
    '/agents': () => import('./routes/app/Agents.svelte'),
    '/fleet': () => import('./routes/app/Fleet.svelte'),
    '/studio': () => import('./routes/app/Studio.svelte'),
    '/templates': () => import('./routes/app/Templates.svelte'),
    '/models': () => import('./routes/app/Models.svelte'),
    '/mcp': () => import('./routes/app/Mcp.svelte'),
    '/observability': () => import('./routes/app/Observability.svelte'),
    '/apps': () => import('./routes/app/Apps.svelte'),
    '/x': {
      '/:app': {
        '/': () => import('./routes/app/XAppView.svelte'),
        '/manage': () => import('./routes/app/XAppManage.svelte'),
      },
    },
    '/settings': () => import('./routes/app/Settings.svelte'),
    '/admin': () => import('./routes/app/Admin.svelte'),
    layout: () => import('./routes/app/AppLayout.svelte'),
    hooks: {
      onError(error) {
        // A lazy chunk that no longer exists after a deploy lands here, as
        // does a throw from a route module's top level. Log it; the shell's
        // <svelte:boundary> is the visible net.
        console.error('[router]', error)
      },
    },
  },
  '*': NotFound,
})

/** Navigate to a server-supplied href (alert / notification / activity links).
 *  The typed `navigate` wants a route-path literal; these arrive as plain
 *  strings (possibly with a query string). One widening cast here instead of
 *  an unsound one at every call site. */
export const navigateHref = (href: string, options?: { replace?: boolean }): Promise<unknown> =>
  (navigate as unknown as (path: string, options?: { replace?: boolean }) => Promise<unknown>)(href, options)
