import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { saveUpload } from '@/server/uploads'

// POST (multipart/form-data, field "file") → store an attachment, return its
// metadata. Any signed-in user may upload; the file is served back from
// /api/uploads/:id.
export const Route = createFileRoute('/api/uploads')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'files.upload'))) return json({ error: 'no permission to upload files' }, { status: 403 })
        const form = await request.formData().catch(() => null)
        const file = form?.get('file')
        if (!(file instanceof File)) return json({ error: 'no file' }, { status: 400 })
        try {
          const bytes = new Uint8Array(await file.arrayBuffer())
          const att = await saveUpload({
            filename: file.name || 'file',
            mime: file.type || 'application/octet-stream',
            bytes,
            userId: user.id,
          })
          return json(att)
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
