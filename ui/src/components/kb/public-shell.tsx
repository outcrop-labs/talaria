import type { ReactNode } from 'react'
import { Brand } from '@/components/brand'

// Chrome for publicly shared KB pages (docs + folders). No app nav, no auth —
// just a clean, centered document with the Talaria mark (spec §4). The content
// column uses the same comfortable measure as the in-app read view.
export function PublicShell({ children, meta }: { children: ReactNode; meta?: string }) {
  return (
    <div className="min-h-screen bg-surface text-fg">
      <header className="border-b border-line-subtle">
        <div className="mx-auto flex max-w-[46rem] items-center gap-2 px-6 py-3">
          <Brand />
          {meta && <span className="ml-auto font-mono text-[10px] uppercase tracking-[0.08em] text-muted">{meta}</span>}
        </div>
      </header>
      <main className="mx-auto max-w-[46rem] px-6 py-12 font-sans text-[0.95rem] leading-[1.7]">{children}</main>
      <footer className="mx-auto max-w-[46rem] px-6 pb-12 pt-6 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
        Shared from a Talaria knowledgebase
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
