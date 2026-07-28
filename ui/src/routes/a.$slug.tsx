import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Markdown } from '@/components/ui/markdown'
import { Skeleton } from '@/components/ui/skeleton'
import { relativeTime } from '@/lib/fleet'
import { PublicShell, PublicNotFound } from '@/components/kb/public-shell'

export const Route = createFileRoute('/a/$slug')({
  component: PublicArtifactPage,
})

interface PublicArtifact {
  kind: string
  title: string
  icon: string | null
  body: string
  updatedAt: string
}

// A publicly shared artifact — no auth. Only artifacts set to public resolve.
function PublicArtifactPage() {
  const { slug } = Route.useParams()
  const [state, setState] = useState<{ a?: PublicArtifact; error?: boolean }>({})
  useEffect(() => {
    fetch(`/api/artifacts/public/${slug}`)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error('not found'))))
      .then((d: { artifact: PublicArtifact }) => setState({ a: d.artifact }))
      .catch(() => setState({ error: true }))
  }, [slug])

  if (state.error) return <PublicNotFound />
  if (!state.a) {
    // First paint for link recipients — doc-page shape regardless of kind.
    return (
      <PublicShell>
        <div aria-hidden>
          <Skeleton className="mb-8 h-8 w-2/3" />
          <div className="space-y-3.5">
            {['100%', '94%', '98%', '88%', '96%', '73%', '100%', '91%', '97%', '85%', '95%', '60%'].map((w, i) => (
              <div key={i} style={{ width: w }}>
                <Skeleton className="h-3.5 w-full rounded-full" delay={i * 0.08} />
              </div>
            ))}
          </div>
        </div>
      </PublicShell>
    )
  }

  // A public microsite is hosted full-bleed in a sandboxed iframe (no app chrome).
  if (state.a.kind === 'microsite') {
    return (
      <div className="min-h-screen bg-white">
        <iframe title={state.a.title} srcDoc={state.a.body} sandbox="allow-scripts allow-forms allow-popups allow-modals" className="h-screen w-full border-0" />
      </div>
    )
  }

  return (
    <PublicShell meta={`Updated ${relativeTime(state.a.updatedAt)}`}>
      <h1 className="mb-5 flex items-center gap-2 text-3xl font-semibold text-fg">
        <span>{state.a.icon ?? '📄'}</span>
        {state.a.title}
      </h1>
      {state.a.kind === 'doc' ? (
        <Markdown className="tiptap">{state.a.body}</Markdown>
      ) : state.a.kind === 'sheet' ? (
        <PublicSheet body={state.a.body} />
      ) : state.a.kind === 'file' ? (
        <a
          href={`/api/artifacts/public/${slug}/download`}
          target="_blank"
          rel="noreferrer"
          className="inline-flex items-center gap-2 rounded-lg border border-line-subtle px-4 py-2 text-sm text-fg hover:border-[var(--theme-accent-border)]"
        >
          ⬇ Download {state.a.title}
        </a>
      ) : (
        <p className="text-sm text-muted">This artifact type isn’t viewable here yet.</p>
      )}
    </PublicShell>
  )
}

function PublicSheet({ body }: { body: string }) {
  let grid: string[][] = []
  try {
    const g = JSON.parse(body)
    if (Array.isArray(g)) grid = g.map((r: unknown[]) => (Array.isArray(r) ? r.map((c) => String(c ?? '')) : []))
  } catch {
    /* empty */
  }
  const [head = [], ...rows] = grid
  return (
    <div className="overflow-x-auto">
      <table className="border-collapse text-sm">
        <thead>
          <tr>{head.map((h, i) => <th key={i} className="border border-line-subtle bg-card px-3 py-1.5 text-left font-semibold text-fg">{h}</th>)}</tr>
        </thead>
        <tbody>
          {rows.map((row, ri) => (
            <tr key={ri}>{row.map((cell, ci) => <td key={ci} className="border border-line-subtle px-3 py-1.5 text-fg">{cell}</td>)}</tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
