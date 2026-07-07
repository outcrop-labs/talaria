import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { Markdown } from '@/components/ui/markdown'
import { relativeTime } from '@/lib/fleet'
import { PublicShell, PublicNotFound } from '@/components/kb/public-shell'

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
  const [state, setState] = useState<{ doc?: PublicDoc; error?: boolean }>({})
  useEffect(() => {
    fetch(`/api/kb/public/${slug}`)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error('not found'))))
      .then((d: { doc: PublicDoc }) => setState({ doc: d.doc }))
      .catch(() => setState({ error: true }))
  }, [slug])

  if (state.error) return <PublicNotFound />
  if (!state.doc) return <PublicShell>{null}</PublicShell>

  return (
    <PublicShell meta={`Updated ${relativeTime(state.doc.updatedAt)}`}>
      <h1 className="mb-5 text-3xl font-semibold text-fg">{state.doc.title}</h1>
      <Markdown className="tiptap">{state.doc.body}</Markdown>
    </PublicShell>
  )
}
