import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Markdown } from '@/components/ui/markdown'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage, getJson, HttpError } from '@/lib/fetch-json'
import { relativeTime } from '@/lib/fleet'
import { PublicShell, PublicNotFound, PublicUnavailable } from '@/components/kb/public-shell'

export const Route = createFileRoute('/kb/$slug')({
  component: PublicDocPage,
})

interface PublicDoc {
  title: string
  body: string
  updatedAt: string
}

// A publicly shared KB doc — no auth. Only docs set to "public" resolve.
function PublicDocPage() {
  const { slug } = Route.useParams()
  // `r.ok ? … : reject('not found')` made EVERY status "not found", including
  // the ones that mean the server is having a bad minute. A visitor with a
  // perfectly good share link was told the page does not exist.
  const [state, setState] = useState<{ doc?: PublicDoc; missing?: boolean; error?: unknown }>({})
  const [reload, setReload] = useState(0)
  useEffect(() => {
    let live = true
    setState({})
    getJson<{ doc: PublicDoc }>(`/api/kb/public/${slug}`)
      .then((d) => live && setState({ doc: d.doc }))
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
  if (!state.doc) {
    // First paint for link recipients — hold the document's shape.
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

  return (
    <PublicShell meta={`Updated ${relativeTime(state.doc.updatedAt)}`}>
      <h1 className="mb-5 font-sans text-3xl font-semibold tracking-tight text-fg">{state.doc.title}</h1>
      <Markdown className="tiptap">{state.doc.body}</Markdown>
    </PublicShell>
  )
}
