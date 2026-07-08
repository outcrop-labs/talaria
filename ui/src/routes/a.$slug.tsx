import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Markdown } from '@/components/ui/markdown'
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
  if (!state.a) return <PublicShell>{null}</PublicShell>

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
      ) : (
        <p className="text-sm text-muted">This artifact type isn’t viewable here yet.</p>
      )}
    </PublicShell>
  )
}
