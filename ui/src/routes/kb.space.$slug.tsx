import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Markdown } from '@/components/ui/markdown'
import { Skeleton } from '@/components/ui/skeleton'
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
  if (!state.space) {
    // First paint for link recipients — hold the overview's shape.
    return (
      <PublicShell>
        <div aria-hidden>
          <Skeleton className="mb-8 h-8 w-2/3" />
          <div className="space-y-3.5">
            {['100%', '94%', '98%', '88%', '96%', '73%', '100%', '91%', '97%', '60%'].map((w, i) => (
              <div key={i} style={{ width: w }}>
                <Skeleton className="h-3.5 w-full rounded-full" delay={i * 0.08} />
              </div>
            ))}
          </div>
        </div>
      </PublicShell>
    )
  }

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
