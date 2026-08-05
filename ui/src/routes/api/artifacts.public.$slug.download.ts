import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getPublicArtifact } from '@/server/artifacts'
import { getUpload } from '@/server/uploads'

// Public download for a public *file* artifact — no auth. Serves the stored
// bytes; only resolves when the artifact is public and points at an upload.
export const Route = defineApi('/api/artifacts/public/$slug/download', {
  GET: async ({ params }) => {
    const a = await getPublicArtifact(params.slug)
    if (!a || a.kind !== 'file' || !a.storageRef) return json({ error: 'not found' }, { status: 404 })
    const up = await getUpload(a.storageRef)
    if (!up) return json({ error: 'not found' }, { status: 404 })
    const inline = /^(image|text)\//.test(up.mime) || up.mime === 'application/pdf'
    return new Response(up.bytes as unknown as BodyInit, {
      headers: {
        'content-type': up.mime,
        'content-disposition': `${inline ? 'inline' : 'attachment'}; filename="${up.filename.replace(/"/g, '')}"`,
        'cache-control': 'public, max-age=3600',
      },
    })
  },
})
