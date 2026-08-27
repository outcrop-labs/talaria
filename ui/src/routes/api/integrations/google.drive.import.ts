import { defineApi } from '@/server/api-route'
import { parseBody, requireUser } from '@/server/api-guard'
import { json } from '@/server/http'
import { z } from 'zod'
import { createArtifact, recordGoogleExport, saveArtifact } from '@/server/artifacts'
import { importDriveFile } from '@/server/google/drive'

const Body = z.object({ fileId: z.string().min(1) })

// POST /api/integrations/google/drive/import { fileId } → pull a Drive file in
// as a new artifact owned by the caller (Doc→doc, Sheet→sheet, else→file).
export const Route = defineApi('/api/integrations/google/drive/import', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const parsed = await parseBody(request, Body)
    if (parsed instanceof Response) return parsed

    const actor = user.email ?? user.name ?? 'user'
    try {
      const content = await importDriveFile(user.id, parsed.fileId, Date.now())
      const artifact = await createArtifact({ kind: content.kind, title: content.title, createdBy: actor, ownerUserId: user.id })
      await saveArtifact(
        artifact.id,
        { title: content.title, body: content.body, storageRef: content.storageRef, contentType: content.contentType },
        actor,
      )
      // Remember where it came from so "Open in Google Drive" links back.
      if (content.sourceUrl) await recordGoogleExport(artifact.id, parsed.fileId, content.sourceUrl)
      return json({ artifact: { ...artifact, kind: content.kind, title: content.title } })
    } catch (err) {
      const e = err as Error
      if (e.name === 'GoogleNotConnected') {
        return json({ error: 'not_connected', message: 'Connect a Google account first.' }, { status: 409 })
      }
      if (/insufficient|ACCESS_TOKEN_SCOPE/i.test(e.message)) {
        return json({ error: 'reconnect_needed', message: 'Reconnect Google to grant Drive read access.' }, { status: 409 })
      }
      if (/too large/i.test(e.message)) {
        return json({ error: 'too_large', message: 'That file is over the 25 MB import limit.' }, { status: 413 })
      }
      if (import.meta.env.DEV) console.error('[drive/import] failed:', e)
      return json({ error: 'import_failed', message: 'Could not import that Drive file.' }, { status: 502 })
    }
  },
})
