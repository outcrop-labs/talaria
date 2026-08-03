import {
  HeadContent,
  Outlet,
  Scripts,
  createRootRoute,
} from '@tanstack/react-router'
import { QueryCache, QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { useState } from 'react'
import appCss from '../styles.css?url'
import { DEFAULT_THEME } from '@/lib/theme'
import { ConfirmHost } from '@/components/ui/confirm'
import { ErrorBoundary } from '@/components/ui/error-boundary'

// Apply the stored Mercury theme before first paint (no flash-of-wrong-theme).
const themeBootScript = `(function(){try{
  var t = localStorage.getItem('talaria-theme');
  if (t !== 'mercury' && t !== 'mercury-light') t = '${DEFAULT_THEME}';
  var m = t === 'mercury' ? 'dark' : 'light';
  var r = document.documentElement;
  r.setAttribute('data-theme', t);
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
    links: [{ rel: 'stylesheet', href: appCss }],
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

  return (
    <html lang="en" data-theme={DEFAULT_THEME} className="dark">
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeBootScript }} />
        <HeadContent />
      </head>
      <body>
        <QueryClientProvider client={queryClient}>
          {/* Outermost net. The router's defaultErrorComponent covers throws
              inside a route; this catches everything above it (providers, the
              shell itself) that would otherwise render an empty <body>. */}
          <ErrorBoundary what="Talaria">
            <Outlet />
            <ConfirmHost />
          </ErrorBoundary>
        </QueryClientProvider>
        <Scripts />
      </body>
    </html>
  )
}
