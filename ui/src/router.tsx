import { createRouter } from '@tanstack/react-router'
import { routeTree } from './routeTree.gen'
import { ErrorFallback } from '@/components/ui/error-boundary'

export const getRouter = () => {
  const router = createRouter({
    routeTree,
    scrollRestoration: true,
    defaultPreloadStaleTime: 0,
    // Every throw inside a route — render, loader, or a lazy chunk that no
    // longer exists after a deploy — lands here instead of white-screening the
    // cockpit. A route may still set its own `errorComponent` to override.
    defaultErrorComponent: ({ error, reset }) => <ErrorFallback error={error} reset={reset} />,
    defaultNotFoundComponent: () => (
      <div className="grid min-h-screen place-items-center p-6 text-center">
        <div>
          <div className="mercury-text mb-1 text-2xl font-semibold">404</div>
          <p className="text-sm text-muted">That view doesn’t exist (yet).</p>
          <a href="/" className="mt-3 inline-block text-sm text-accent hover:underline">
            Back to chat
          </a>
        </div>
      </div>
    ),
  })
  return router
}

declare module '@tanstack/react-router' {
  interface Register {
    router: ReturnType<typeof getRouter>
  }
}
