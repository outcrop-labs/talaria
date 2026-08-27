import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { actorOf, requireUser } from '@/server/api-guard'
import { agentCaller, refuseLegacy } from '@/server/agent-auth'
import { logAudit } from '@/server/audit'
import { getArtifact, guarded, recordGoogleExport } from '@/server/artifacts'
import { canRead, listEditors } from '@/server/kb-perms'
import { isConnected } from '@/server/google/connections'
import { exportArtifactToDrive, exportArtifactWithToken } from '@/server/google/drive'
import { resolveAgentGoogle } from '@/server/google/agent-google'
import { getOrgTargets } from '@/server/google/org-connection'

// POST /api/artifacts/$id/export/google — mirror an artifact into Google Drive.
//
// Whose Drive it lands in depends on the caller (per-user OAuth):
//   human            → their own connected Drive
//   personal agent   → its OWNER's Drive (acts as the human it works for)
//   general agent    → the shared ORG Drive (no human owner of its own)
export const Route = defineApi('/api/artifacts/$id/export/google', {
  POST: async ({ request, params }) => {
    const artifact = await getArtifact(params.id)
    if (!artifact) return json({ error: 'not found' }, { status: 404 })
    const editors = await listEditors('artifact', artifact.id)

    try {
      let file
      const agent = await agentCaller(request)
      if (agent instanceof Response) return agent
      if (agent) {
        // Writing into a human's Drive (personal assistant) or the shared
        // ORG Drive is acting AS someone — the same grant the Gmail and
        // Calendar agent routes refuse to a legacy caller, and this one
        // must too. The shared org key proves fleet membership, not
        // identity, so "I am <any ordinary agent>" cannot buy org Drive
        // write access.
        const denied = refuseLegacy(agent, 'Google Drive export')
        if (denied) return denied
        const name = agent.model
        const allowed =
          artifact.visibility !== 'private' || editors.some((e) => e.principalType === 'agent' && e.principalId === name)
        if (!allowed) return json({ error: 'forbidden' }, { status: 403 })
        // Resolve the agent's Google identity (owner for personal assistants,
        // shared org account for general fleet agents). Pass the CALLER —
        // resolveAgentGoogle re-checks proof, so this route's guard is a
        // layer rather than the only one.
        const google = await resolveAgentGoogle(agent, Date.now())
        if (!google) {
          return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
        }
        // Org files go to the configured Shared Drive/folder (team-owned).
        const folderId = google.principal === 'org' ? (await getOrgTargets()).driveFolderId : null
        file = await exportArtifactWithToken(google.token, artifact, { folderId })
      } else {
        const gate = await requireUser(request)
        if (gate instanceof Response) return gate
        const user = gate
        if (!canRead(guarded(artifact), user.id, user.email ?? user.name, editors)) {
          return json({ error: 'forbidden' }, { status: 403 })
        }
        if (!(await isConnected(user.id))) {
          return json({ error: 'not_connected', message: 'Connect a Google account first (Settings → Integrations).' }, { status: 409 })
        }
        file = await exportArtifactToDrive(user.id, artifact, Date.now())
        void logAudit({ actor: actorOf(user), action: 'artifact.export_google', targetType: 'artifact', targetId: params.id, targetLabel: artifact.title })
      }

      await recordGoogleExport(artifact.id, file.id, file.url)
      return json({ file })
    } catch (err) {
      const e = err as Error
      if (e.name === 'GoogleNotConnected') {
        return json({ error: 'not_connected', message: 'Connect a Google account first.' }, { status: 409 })
      }
      if (e.name === 'NotExportable') {
        // 400, not 422: the artifact's kind is the client's own choice, so an
        // unexportable one is a malformed request for THIS endpoint — the
        // repo's contract is 400 for shape problems and 422 never appears.
        return json({ error: 'not_exportable', message: 'This artifact can’t be exported to Google Drive.' }, { status: 400 })
      }
      if (import.meta.env.DEV) console.error('[artifacts/export/google] failed:', e)
      return json({ error: 'export_failed', message: 'Google Drive rejected the export.' }, { status: 502 })
    }
  },
})
