import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { getArtifact, guarded, recordGoogleExport } from '@/server/artifacts'
import { canRead, listEditors } from '@/server/kb-perms'
import { isConnected } from '@/server/google/connections'
import { exportArtifactToDrive } from '@/server/google/drive'

// POST /api/artifacts/$id/export/google — mirror an artifact into Google Drive.
//
// Per-user OAuth: the file is created in the Google account of whoever's
// connection we use. A human exports into their OWN Drive; an agent exports into
// its artifact OWNER's Drive (identity proxy — the agent acts as the human it
// works for), never its own, since agents have no Google account.
export const Route = createFileRoute('/api/artifacts/$id/export/google')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const artifact = await getArtifact(params.id)
        if (!artifact) return json({ error: 'not found' }, { status: 404 })
        const editors = await listEditors('artifact', artifact.id)

        // Resolve WHO reads and WHOSE Drive we write to.
        let driveUserId: string | null
        if (checkAgentKey(request)) {
          const name = agentName(request)
          const allowed =
            artifact.visibility !== 'private' || editors.some((e) => e.principalType === 'agent' && e.principalId === name)
          if (!name || !allowed) return json({ error: 'forbidden' }, { status: 403 })
          // The agent acts as the artifact's owner.
          driveUserId = artifact.ownerUserId
          if (!driveUserId) return json({ error: 'no_owner', message: 'This artifact has no human owner whose Drive to export into.' }, { status: 409 })
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!canRead(guarded(artifact), user.id, user.email ?? user.name, editors)) {
            return json({ error: 'forbidden' }, { status: 403 })
          }
          driveUserId = user.id
        }

        if (!(await isConnected(driveUserId))) {
          return json({ error: 'not_connected', message: 'Connect a Google account first (Settings → Integrations).' }, { status: 409 })
        }

        try {
          const file = await exportArtifactToDrive(driveUserId, artifact, Date.now())
          await recordGoogleExport(artifact.id, file.id, file.url)
          return json({ file })
        } catch (err) {
          const e = err as Error
          if (e.name === 'GoogleNotConnected') {
            return json({ error: 'not_connected', message: 'Connect a Google account first.' }, { status: 409 })
          }
          if (e.name === 'NotExportable') {
            return json({ error: 'not_exportable', message: 'This artifact can’t be exported to Google Drive.' }, { status: 422 })
          }
          if (import.meta.env.DEV) console.error('[artifacts/export/google] failed:', e)
          return json({ error: 'export_failed', message: 'Google Drive rejected the export.' }, { status: 502 })
        }
      },
    },
  },
})
