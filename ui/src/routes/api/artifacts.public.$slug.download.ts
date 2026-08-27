import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getPublicArtifact } from '@/server/artifacts'
import { getUpload, serveUpload } from '@/server/uploads'

// Public download for a public *file* artifact — no auth. Serves the stored
// bytes; only resolves when the artifact is public and points at an upload.
// The inline/download decision lives in serveUpload (server/uploads.ts) — this
// route is UNAUTHENTICATED, so it especially may not widen that allowlist.
export const Route = defineApi('/api/artifacts/public/$slug/download', {
  GET: async ({ params }) => {
    const a = await getPublicArtifact(params.slug)
    if (!a || a.kind !== 'file' || !a.storageRef) return json({ error: 'not found' }, { status: 404 })
    const up = await getUpload(a.storageRef)
    if (!up) return json({ error: 'not found' }, { status: 404 })
    return serveUpload(up, { cache: 'public, max-age=3600' })
  },
})
