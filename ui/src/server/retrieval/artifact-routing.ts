// Artifact ↔ brain placement — the same routing control KB docs have.
//   auto   the artifact's normal flows (plan-doc / research activity indexing,
//          officialize → KB mirror) decide where it's retrievable
//   none   never indexed anywhere — scrub every copy
//   <id>   explicit custom-brain assignment: it lives ONLY there
// Privacy trumps routing: a private artifact never lands in a shared brain.
import { artifactToMarkdown, targetsForArtifact, type Artifact } from '../artifacts'
import { indexPlanDoc } from '../plan-doc'
import { db } from '../db/pg'
import { indexDocument, unindexDocument, type DocAcl } from './index'
import { indexActivity, indexPersonal, unindexActivity, unindexPersonal } from './sources'

/** Re-place an artifact according to its routing. Idempotent; call after any
 *  routing change (and the backfill/sweep call it for non-auto artifacts). */
export async function applyArtifactRouting(artifact: Artifact): Promise<void> {
  const sql = await db()
  // Scrub explicit-brain copies first (re-routing must not leave stale ones).
  const customs = (await sql`select id from rag_collections where kind = 'custom'`) as unknown as Array<{ id: string }>
  for (const c of customs) await unindexDocument(c.id, 'artifact', artifact.id).catch(() => {})

  if (artifact.ragRouting === 'none') {
    // Scrub the copies its auto flows may have created.
    await unindexActivity('plan-doc', artifact.id).catch(() => {})
    await unindexActivity('research', artifact.id).catch(() => {})
    if (artifact.ownerUserId) await unindexPersonal(artifact.ownerUserId, 'research', artifact.id).catch(() => {})
    return
  }

  if (artifact.ragRouting !== 'auto') {
    // Explicit brain: it lives only there — auto-flow copies go too.
    await unindexActivity('plan-doc', artifact.id).catch(() => {})
    await unindexActivity('research', artifact.id).catch(() => {})
    if (artifact.ownerUserId) await unindexPersonal(artifact.ownerUserId, 'research', artifact.id).catch(() => {})
    if (artifact.visibility === 'private') return // privacy trumps routing
    const text = artifactToMarkdown(artifact)
    if (!text.trim()) return // files have no text body
    await indexDocument(artifact.ragRouting, {
      sourceType: 'artifact',
      sourceId: artifact.id,
      title: artifact.title,
      text: `${artifact.title}\n\n${text}`,
      // Item ACL — a custom brain holds items of mixed visibility, so the
      // document-scope filter re-checks this at query time.
      payload: { visibility: artifact.visibility, ownerUserId: artifact.ownerUserId ?? null } satisfies DocAcl,
      href: '/artifacts',
    }).catch(() => {})
    return
  }

  // Back to auto: restore the flows that would have indexed it.
  const targets = await targetsForArtifact(artifact.id)
  const plan = targets.find((t) => t.targetType === 'plan')
  const research = targets.find((t) => t.targetType === 'research')
  if (plan) {
    await indexPlanDoc(artifact, plan.targetId).catch(() => {})
  } else if (research) {
    // Personal research lives in the owner's private brain; org research in
    // the ambient index, marked orgWide so scopes match it.
    const doc = {
      sourceType: 'research',
      sourceId: artifact.id,
      title: artifact.title,
      text: `${artifact.title}\n\n${artifact.body}`,
      payload: artifact.ownerUserId ? { runId: research.targetId } : { runId: research.targetId, orgWide: true },
      href: `/research?r=${research.targetId}`,
    }
    if (artifact.visibility === 'private' && artifact.ownerUserId) await indexPersonal(artifact.ownerUserId, doc).catch(() => {})
    else if (artifact.visibility !== 'private') await indexActivity(doc).catch(() => {})
  }
}
