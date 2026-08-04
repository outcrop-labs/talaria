import type { ReactNode } from 'react'

// Chrome for publicly shared KB pages (docs + folders). No app nav, no auth —
// just a clean, centered document with a small Talaria mark. The content column
// uses the same comfortable measure as the in-app read view.
export function PublicShell({ children, meta }: { children: ReactNode; meta?: string }) {
  return (
    <div className="min-h-screen bg-surface text-fg">
      <header className="border-b border-line-subtle">
        <div className="mx-auto flex max-w-[46rem] items-center gap-2 px-6 py-3 text-sm text-muted">
          <span className="text-accent">❖</span>
          <span className="font-medium">Talaria</span>
          {meta && <span className="ml-auto text-xs">{meta}</span>}
        </div>
      </header>
      <main className="mx-auto max-w-[46rem] px-6 py-12 text-[0.95rem] leading-[1.7]">{children}</main>
      <footer className="mx-auto max-w-[46rem] px-6 pb-12 pt-6 text-xs text-muted">
        Shared from a Talaria knowledgebase.
      </footer>
    </div>
  )
}

export function PublicNotFound() {
  return (
    <PublicShell>
      <div className="py-16 text-center">
        <div className="mb-2 text-2xl font-semibold text-fg">Not found</div>
        <p className="text-sm text-muted">This page isn’t shared publicly, or the link has changed.</p>
      </div>
    </PublicShell>
  )
}

/** The OTHER failure. All three public routes used to `.catch(() => error)` and
 *  render `PublicNotFound` for every status, so a 500 on a share link told a
 *  visitor — someone outside the org, who cannot check — that the page does not
 *  exist. That is the one message a working link must never produce: the sender
 *  gets told their link is broken and reshares or rebuilds the document.
 *  A 404 is still "Not found". Everything else says come back, and offers a
 *  retry, because everything else is temporary. */
export function PublicUnavailable({ onRetry, detail }: { onRetry?: () => void; detail?: string }) {
  return (
    <PublicShell>
      <div className="py-16 text-center">
        <div className="mb-2 text-2xl font-semibold text-fg">This page isn’t loading right now</div>
        <p className="text-sm text-muted">
          The link is fine — the server didn’t answer. Try again in a moment.
        </p>
        {detail && <p className="mt-2 text-xs text-muted">{detail}</p>}
        {onRetry && (
          <button
            type="button"
            onClick={onRetry}
            className="mt-5 rounded-lg border border-line px-4 py-2 text-sm text-fg transition-colors hover:border-[var(--theme-accent-border)]"
          >
            Try again
          </button>
        )}
      </div>
    </PublicShell>
  )
}
