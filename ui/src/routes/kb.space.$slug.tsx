import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Markdown } from '@/components/ui/markdown'
import { PublicShell, PublicNotFound } from '@/components/kb/public-shell'

export const Route = createFileRoute('/kb/space/$slug')({
  component: PublicSpacePage,
})

interface PublicSpace {
  name: string
  icon: string | null
  body: string
}

// A publicly shared KB folder — no auth. Shows the folder's overview.
function PublicSpacePage() {
  const { slug } = Route.useParams()
  const [state, setState] = useState<{ space?: PublicSpace; error?: boolean }>({})
  useEffect(() => {
    fetch(`/api/kb/public/space/${slug}`)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error('not found'))))
      .then((d: { space: PublicSpace }) => setState({ space: d.space }))
      .catch(() => setState({ error: true }))
  }, [slug])

  if (state.error) return <PublicNotFound />
  if (!state.space) return <PublicShell>{null}</PublicShell>

  return (
    <PublicShell>
      <h1 className="mb-5 flex items-center gap-2 text-3xl font-semibold text-fg">
        <span>{state.space.icon ?? '📚'}</span>
        {state.space.name}
      </h1>
      {state.space.body.trim() ? (
        <Markdown className="tiptap">{state.space.body}</Markdown>
      ) : (
        <p className="text-sm text-muted">This folder doesn’t have an overview yet.</p>
      )}
    </PublicShell>
  )
}
