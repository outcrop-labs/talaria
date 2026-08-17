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
    // Home's console tabs. Under /home rather than at the root because their
    // names ARE the names of real views: /boards is the board list, /home/boards
    // is Home's summary of it.
    '/home': {
      '/:tab': () => import('./routes/app/Home.svelte'),
    },
    '/chat': () => import('./routes/app/Chat.svelte'),
    // A DISCRIMINATED path. The selection is a tagged union — a channel, or a
    // thread with an agent — so the tag is a segment: the two kinds cannot be
    // confused, and a thread hangs off the agent that owns it.
    '/comms': {
      '/': () => import('./routes/app/Comms.svelte'),
      '/channel': {
        '/:id': () => import('./routes/app/Comms.svelte'),
      },
      '/agent': {
        '/:model': {
          '/': () => import('./routes/app/Comms.svelte'),
          '/:thread': () => import('./routes/app/Comms.svelte'),
        },
      },
    },
    '/channels': () => import('./routes/app/Channels.svelte'),
    '/inbox': () => import('./routes/app/Inbox.svelte'),
    '/boards': {
      '/': () => import('./routes/app/boards/BoardsIndex.svelte'),
      '/:boardId': {
        // BoardLayout persists across board↔ticket navigation (sv-router
        // layouts survive child swaps), so opening a ticket mounts ONLY the
        // overlay — the board behind it never re-renders.
        '/': () => import('./routes/app/boards/NoOverlay.svelte'),
        '/:taskId': () => import('./routes/app/boards/Task.svelte'),
        layout: () => import('./routes/app/boards/BoardLayout.svelte'),
      },
    },
    // The selected plan / run is a path, not a query: it is the thing the page
    // is about, and a link to it should look like one.
    '/plan': {
      '/': () => import('./routes/app/Plan.svelte'),
      '/:planId': () => import('./routes/app/Plan.svelte'),
    },
    '/research': {
      '/': () => import('./routes/app/Research.svelte'),
      '/:runId': () => import('./routes/app/Research.svelte'),
    },
    // Space then doc — genuinely hierarchical, so it reads as a path.
    // `/knowledge?doc=<id>` still resolves: it is the by-id permalink for
    // links that know a document but not which space it lives in.
    '/knowledge': {
      '/': () => import('./routes/app/Knowledge.svelte'),
      '/:space': {
        '/': () => import('./routes/app/Knowledge.svelte'),
        '/:doc': () => import('./routes/app/Knowledge.svelte'),
      },
    },
    // PLACE is the path; the folder you are browsing and the file you have
    // open stay in the query. They are a different axis — selection WITHIN a
    // place — and they are independent of each other, so a file can be open
    // with no folder. A positional path could not say that without a
    // placeholder segment.
    '/artifacts': {
      '/': () => import('./routes/app/Artifacts.svelte'),
      '/:place': () => import('./routes/app/Artifacts.svelte'),
    },
    // Each tab is a real location: /agents (roster), /agents/templates,
    // /agents/schedules. A tab you cannot link to, bookmark, or reach with the
    // back button is a mode, not a place.
    '/agents': {
      '/': () => import('./routes/app/Agents.svelte'),
      '/:tab': () => import('./routes/app/Agents.svelte'),
    },
    '/fleet': () => import('./routes/app/Fleet.svelte'),
    '/studio': () => import('./routes/app/Studio.svelte'),
    '/templates': {
      '/': () => import('./routes/app/Templates.svelte'),
      '/:tab': () => import('./routes/app/Templates.svelte'),
    },
    '/models': {
      '/': () => import('./routes/app/Models.svelte'),
      '/:tab': () => import('./routes/app/Models.svelte'),
    },
    '/mcp': {
      '/': () => import('./routes/app/Mcp.svelte'),
      '/:tab': () => import('./routes/app/Mcp.svelte'),
    },
    '/observability': {
      '/': () => import('./routes/app/Observability.svelte'),
      '/:tab': () => import('./routes/app/Observability.svelte'),
    },
    '/apps': () => import('./routes/app/Apps.svelte'),
    '/x': {
      '/:app': {
        '/': () => import('./routes/app/XAppView.svelte'),
        '/manage': () => import('./routes/app/XAppManage.svelte'),
      },
    },
    '/settings': {
      '/': () => import('./routes/app/Settings.svelte'),
      '/:tab': () => import('./routes/app/Settings.svelte'),
    },
    '/admin': {
      '/': () => import('./routes/app/Admin.svelte'),
      '/:tab': () => import('./routes/app/Admin.svelte'),
    },
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
