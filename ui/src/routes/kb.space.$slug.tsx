import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Markdown } from '@/components/ui/markdown'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage, getJson, HttpError } from '@/lib/fetch-json'
import { PublicShell, PublicNotFound, PublicUnavailable } from '@/components/kb/public-shell'

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
  // `r.ok ? … : reject('not found')` made EVERY status "not found", including
  // the ones that mean the server is having a bad minute. A visitor with a
  // perfectly good share link was told the page does not exist.
  const [state, setState] = useState<{ space?: PublicSpace; missing?: boolean; error?: unknown }>({})
  const [reload, setReload] = useState(0)
  useEffect(() => {
    let live = true
    setState({})
    getJson<{ space: PublicSpace }>(`/api/kb/public/space/${slug}`)
      .then((d) => live && setState({ space: d.space }))
      .catch((e: unknown) => {
        if (!live) return
        // 404 is the only status that means "there is no such page".
        setState(e instanceof HttpError && e.status === 404 ? { missing: true } : { error: e })
      })
    return () => {
      live = false
    }
  }, [slug, reload])

  if (state.missing) return <PublicNotFound />
  if (state.error)
    return <PublicUnavailable detail={errorMessage(state.error)} onRetry={() => setReload((n) => n + 1)} />
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
      <h1 className="mb-5 flex items-center gap-2 font-sans text-3xl font-semibold tracking-tight text-fg">
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
