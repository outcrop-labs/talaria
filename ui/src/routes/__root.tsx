import {
  HeadContent,
  Outlet,
  Scripts,
  createRootRoute,
} from '@tanstack/react-router'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { MotionConfig } from 'framer-motion'
import { useState } from 'react'
import appCss from '../styles.css?url'
import { DEFAULT_THEME } from '@/lib/theme'
import { ConfirmHost } from '@/components/ui/confirm'

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
          {/* Spec §9 reduced-motion rule for the framer-motion side of the
              grammar: entrances lose travel/scale and degrade to fade-only
              when the OS asks for reduced motion (CSS motifs are handled by
              the prefers-reduced-motion block in styles.css). */}
          <MotionConfig reducedMotion="user">
            <Outlet />
            <ConfirmHost />
          </MotionConfig>
        </QueryClientProvider>
        <Scripts />
      </body>
    </html>
  )
}
