// The whole route map, explicit (sv-router code-based tree mode). Its own
// module, apart from router.ts, so a node test can run sv-router's matcher
// over the real table (route-table.test.ts): router.ts calls createRouter,
// which reaches for `document` the moment it runs. Lazy `() => import(...)`
// per view keeps code splitting; the app shell (AppLayout) is the layout for
// everything behind the session gate — and it is lazy TOO (the old _app.tsx
// was a lazy file route for the same reason): eager, it hoists the whole
// shell graph — InboxFocusShell → InboxChatPanel → RichEditor →
// tiptap/prosemirror — into the entry chunk that /login and the public share
// pages must also download.
//
// Old TanStack file routes → this tree:
//   routes/login.tsx            → '/login'
//   routes/join.tsx             → '/join'
//   routes/a.$slug.tsx          → '/a/:slug'      (public artifact share)
//   routes/kb.$slug.tsx         → '/kb/:slug'     (public doc share)
//   routes/kb.space.$slug.tsx   → '/kb/space/:slug'
//   routes/_app.tsx             → layout: AppLayout
//   routes/_app/<view>.tsx      → '/<view>'
//   routes/_app/x.$app.*.tsx    → '/x/:app'(+/manage, and the routes below each)
//   routes/_app/boards/*        → '/boards', '/boards/:boardId(/:taskId)'
//
// CONFIG KEYS ARE MATCHED AS PATH SEGMENTS. sv-router reads `layout`, `hooks`
// and `meta` as route keys too: on /x/hooks, a `hooks` key beside the slug
// segment wins over '/:app' (a static key outranks a dynamic one), resolves
// to no route, and the matcher throws — a blank page, a dead dock tile. So no
// config key sits in '/x' or anywhere below it, and the shell's layout and
// hooks stay one level up, beside '/x', where they face only the segment `x`.
// Install refuses those three words as slugs too (RESERVED_APP_SLUGS in
// talaria-users); this table is what keeps an app that already has one
// working. route-table.test.ts runs the matcher over this table for the
// shared fixture's routes and fails if a config key lands below '/x'.
import type { Routes } from 'sv-router'
import NotFound from './routes/NotFound.svelte'

/** The app shell, as the layout of everything behind the session gate. */
export const shell = () => import('./routes/app/AppLayout.svelte')
const shellHooks = {
  onError(error: unknown) {
    // A lazy chunk that no longer exists after a deploy lands here, as
    // does a throw from a route module's top level. Log it; the shell's
    // <svelte:boundary> is the visible net.
    console.error('[router]', error)
  },
}

/** An app's work surface, mounted at /x/<slug> and every route below it. */
export const appWorkSurface = () => import('./routes/app/XAppView.svelte')
/** An app's manage surface, mounted at /x/<slug>/manage and every route below it. */
export const appManageSurface = () => import('./routes/app/XAppManage.svelte')

export { NotFound }

export const routes = {
  '/login': () => import('./routes/Login.svelte'),
  '/claim': () => import('./routes/Claim.svelte'),
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
    // The landing bounces to the Inbox's own URL. `/` used to BE the Inbox,
    // which left the surface with no address of its own — you could not link to
    // it, and the nav rail entry for it was a root path that is an ancestor of
    // every other route (see `activeAmong` in lib/route-tabs.ts for what that
    // cost). Home is the container; the Inbox is a route inside it.
    '/': () => import('./routes/app/HomeRedirect.svelte'),
    // Home and its tabs. Under /home rather than at the root because their
    // names ARE the names of real views: /boards is the board list, /home/boards
    // is Home's summary of it.
    '/home': {
      // Bare `/home` renders the default tab (the Inbox) rather than 404ing, so
      // the nav rail can point at the container.
      '/': () => import('./routes/app/Home.svelte'),
      '/:tab': () => import('./routes/app/Home.svelte'),
    },
    '/chat': () => import('./routes/app/Chat.svelte'),
    // A DISCRIMINATED path. The selection is a tagged union — a channel, or a
    // thread with an agent — so the tag is a segment: the two kinds cannot be
    // confused, and a thread hangs off the agent that owns it.
    '/comms': {
      '/': () => import('./routes/app/Comms.svelte'),
      // The sidebar's two cross-conversation views (Threads, Drafts & sent).
      '/threads': () => import('./routes/app/Comms.svelte'),
      '/drafts': () => import('./routes/app/Comms.svelte'),
      '/new': () => import('./routes/app/Comms.svelte'),
      // Every conversation shared with one person or agent (the profile
      // drawer's "See all conversations"): /comms/with/<person|agent>/<id>.
      '/with': {
        '/:kind': {
          '/:id': () => import('./routes/app/Comms.svelte'),
        },
      },
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
      layout: () => import('./routes/app/boards/BoardsShell.svelte'),
      '/': () => import('./routes/app/boards/BoardsIndex.svelte'),
      '/:boardId': {
        // BoardLayout persists across board↔ticket navigation (sv-router
        // layouts survive child swaps), so opening a ticket mounts ONLY the
        // overlay — the board behind it never re-renders. BoardsShell wraps
        // it: the boards view's own sidebar lives at THIS level, so the board
        // and the ticket overlay both get it and the shell's nav does not.
        '/': () => import('./routes/app/boards/NoOverlay.svelte'),
        '/:taskId': () => import('./routes/app/boards/Task.svelte'),
        layout: () => import('./routes/app/boards/BoardLayout.svelte'),
      },
    },
    // ONE SURFACE FOR EVERY SESSION, at three addresses. Work, Plan and
    // Research were three views of one shape and are now the work harness:
    // `/work` is the session list, `/work/:sessionId` is one session.
    //
    // The other two paths still resolve, because a URL somebody linked,
    // bookmarked or was mailed is a promise. `/plan/:planId` names a session
    // id, and the harness reads the selection out of either path.
    // `/research/:runId` names a RUN rather than a conversation, so the
    // harness resolves the run's own discussion and replaces the address with
    // `/work/<conversationId>` — or, for a run that has no discussion to open,
    // renders its report alone. A bare `/plan` or `/research` is the harness
    // with nothing selected; the first selection moves the URL to `/work`.
    '/work': {
      '/': () => import('./routes/app/WorkHarness.svelte'),
      '/:sessionId': () => import('./routes/app/WorkHarness.svelte'),
    },
    '/plan': {
      '/': () => import('./routes/app/WorkHarness.svelte'),
      '/:planId': () => import('./routes/app/WorkHarness.svelte'),
    },
    '/research': {
      '/': () => import('./routes/app/WorkHarness.svelte'),
      '/:runId': () => import('./routes/app/WorkHarness.svelte'),
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
    '/teams': () => import('./routes/app/Teams.svelte'),
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
    // The control plane's front door: what needs attention across agents,
    // models, MCP, spend and apps, with each view's own page one click away
    // in the sidebar docked beside it.
    '/manage': () => import('./routes/app/Manage.svelte'),
    '/settings': {
      '/': () => import('./routes/app/Settings.svelte'),
      '/:tab': () => import('./routes/app/Settings.svelte'),
    },
    '/admin': {
      '/': () => import('./routes/app/Admin.svelte'),
      '/:tab': () => import('./routes/app/Admin.svelte'),
    },
    // AN APP OWNS EVERY ROUTE BELOW ITS SURFACES. /x/<slug>/<rest> is the
    // work surface and /x/<slug>/manage/<rest> the manage surface, each
    // mounted once with <rest> handed to it (AppSurface → useAppRoute), so an
    // app's list, its record pages and its tabs are real, linkable addresses.
    // The route gate reads the same split (appViewFor in lib/app-grants.ts):
    // a work grant opens /x/<slug>/a/b and never /x/<slug>/manage/a. A
    // settings tab is one page and has no routes below it.
    //
    // No layout, hooks or meta key in here — see CONFIG KEYS above.
    //
    // The catch-alls are bare '*', not '*rest': a named one makes sv-router
    // decode the rest, and a malformed escape (/x/crm/%zz) makes its matcher
    // throw a URIError, so the navigation fails. AppSurface reads the rest
    // off the pathname and the SDK decodes it once, per segment, keeping a
    // malformed escape as spelled.
    '/x': {
      '/:app': {
        '/': appWorkSurface,
        '/manage': {
          '/': appManageSurface,
          '*': appManageSurface,
        },
        '*': appWorkSurface,
      },
    },
    // NOT FOUND LIVES IN HERE, OUT OF THE SHELL. sv-router takes a catch-all
    // inside this pathless group only as a FALLBACK, and a top-level '*'
    // would beat it — /x/<slug>/items rendered Not Found while one stood
    // there. With the 404 in here as '(*)' (a catch-all that breaks out of
    // the layouts above it), the more specific '/x' catch-alls win, an
    // unknown path still renders Not Found bare (no shell, no session gate),
    // and a path under /login, /a or /kb that matches nothing falls back to
    // it as before.
    '(*)': NotFound,
    layout: shell,
    hooks: shellHooks,
  },
} satisfies Routes
