import {
  HeadContent,
  Outlet,
  Scripts,
  createRootRoute,
} from '@tanstack/react-router'
import { QueryCache, QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MotionConfig } from 'framer-motion'
import { useState } from 'react'
import appCss from '../styles.css?url'
import { DEFAULT_THEME, isDarkTheme } from '@/lib/theme'
import { ConfirmHost } from '@/components/ui/confirm'
import { ErrorBoundary } from '@/components/ui/error-boundary'

// Vite's dev server appends an HMR cache-buster (`?t=<timestamp>`) to the
// client-side URL once styles.css has been edited while the server runs, but
// the SSR module graph keeps its own URL. React 19 keys stylesheet links by
// href, so the mismatched <link> is a node-level hydration failure that
// regenerates the whole tree on every route. The query adds nothing (Vite
// serves source files with no-cache + ETag, and the production URL is a
// content-hashed filename with no query), so pin the stable URL on both sides.
const appCssHref = appCss.split('?')[0]!

// Apply the stored Mercury theme before first paint (no flash-of-wrong-theme).
// Mirrors applyTheme() in lib/theme.ts: swap the mode class (never stack
// 'dark light'), then stamp data-theme + color-scheme.
const themeBootScript = `(function(){try{
  var t = localStorage.getItem('talaria-theme');
  if (t !== 'mercury' && t !== 'mercury-light') t = '${DEFAULT_THEME}';
  var m = t === 'mercury' ? 'dark' : 'light';
  var r = document.documentElement;
  r.setAttribute('data-theme', t);
  r.classList.remove('light', 'dark');
  r.classList.add(m);
  r.style.setProperty('color-scheme', m);
}catch(e){}})();`

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: 'utf-8' },
      { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      { title: 'Talaria — the winged fleet cockpit' },
      { name: 'color-scheme', content: 'dark light' },
    ],
    links: [{ rel: 'stylesheet', href: appCssHref }],
  }),
  shellComponent: RootDocument,
})

function RootDocument() {
  const [queryClient] = useState(
    () =>
      new QueryClient({
        // A query that errors used to fail completely silently — the incident
        // where a 500 from /api/boards rendered as "no boards" left no trace
        // anywhere. This does not change behaviour, it just makes a failed
        // fetch visible in the console with the key that failed.
        queryCache: new QueryCache({
          onError: (error, query) => {
            console.error('[query]', JSON.stringify(query.queryKey), error)
          },
        }),
        defaultOptions: { queries: { staleTime: 5_000, retry: 1 } },
      }),
  )

  const defaultMode = isDarkTheme(DEFAULT_THEME) ? 'dark' : 'light'

  return (
    // suppressHydrationWarning (one element deep, <html> only): the boot
    // script above intentionally retunes data-theme / mode class / inline
    // color-scheme before React hydrates, so the server-rendered attributes
    // legitimately differ from the client's. Without it React logs "A tree
    // hydrated but some attributes ... didn't match" on every route. SSR
    // still emits the complete default-theme attribute set (including the
    // inline color-scheme the script would add) so the default case matches
    // byte-for-byte and suppression only covers a stored non-default theme.
    <html
      lang="en"
      data-theme={DEFAULT_THEME}
      className={defaultMode}
      style={{ colorScheme: defaultMode }}
      suppressHydrationWarning
    >
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeBootScript }} />
        <HeadContent />
      </head>
      <body>
        <QueryClientProvider client={queryClient}>
          {/* Outermost net. The router's defaultErrorComponent covers throws
              inside a route; this catches everything above it (providers, the
              shell itself) that would otherwise render an empty <body>. It
              stays OUTSIDE MotionConfig so a throw from the motion layer
              itself still lands somewhere visible. */}
          <ErrorBoundary what="Talaria">
            {/* Spec §9 reduced-motion rule for the framer-motion side of the
                grammar: entrances lose travel/scale and degrade to fade-only
                when the OS asks for reduced motion (CSS motifs are handled by
                the prefers-reduced-motion block in styles.css). */}
            <MotionConfig reducedMotion="user">
              <Outlet />
              <ConfirmHost />
            </MotionConfig>
          </ErrorBoundary>
        </QueryClientProvider>
        <Scripts />
      </body>
    </html>
  )
}
