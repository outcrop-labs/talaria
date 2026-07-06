import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { getUpload } from '@/server/uploads'

// GET → serve an attachment's bytes (signed-in users only). Images render
// inline in the client; everything else downloads.
export const Route = createFileRoute('/api/uploads/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const up = await getUpload(params.id)
        if (!up) return json({ error: 'not found' }, { status: 404 })
        const inline = /^(image|text)\//.test(up.mime) || up.mime === 'application/pdf'
        return new Response(up.bytes as unknown as BodyInit, {
          headers: {
            'content-type': up.mime,
            'content-disposition': `${inline ? 'inline' : 'attachment'}; filename="${up.filename.replace(/"/g, '')}"`,
            'cache-control': 'private, max-age=86400',
          },
        })
      },
    },
  },
})
