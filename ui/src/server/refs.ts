// Message references — attaching KNOWLEDGE and ARTIFACTS to chat/channel
// messages, not just file uploads. A ref persists as a chip on the message
// (attachments jsonb, refType set) carrying a clipped copy of the content, so
// the model sees the material on the turn it was attached AND on every later
// history rebuild (queued turns, resumes, channel transcripts) without
// re-fetching. ACL-checked against the ATTACHER at attach time.
import { artifactToMarkdown, getArtifact, guarded } from './artifacts'
import { effectiveDocPerms, getDoc } from './kb'
import { canRead, listEditors } from './kb-perms'

export interface MessageRef {
  type: 'kb-doc' | 'artifact'
  id: string
}

/** The chip persisted into the message's attachments array. */
export interface RefChip {
  id: string
  filename: string
  mime: string // 'ref/kb-doc' | 'ref/artifact'
  size: number
  refType: 'kb-doc' | 'artifact'
  /** Clipped content the model reads; never rendered in the UI. */
  content: string
}

const CLIP = 6_000
const clip = (s: string) => (s.length > CLIP ? `${s.slice(0, CLIP)}\n[clipped]` : s)

/** Resolve + ACL-check refs for a user. Unknown/forbidden refs are dropped
 *  silently — attaching must never leak whether a private thing exists. */
export async function resolveRefs(
  user: { id: string; email?: string | null; name?: string | null },
  refs: MessageRef[],
): Promise<RefChip[]> {
  const chips: RefChip[] = []
  for (const ref of refs.slice(0, 3)) {
    if (ref.type === 'kb-doc') {
      const doc = await getDoc(ref.id).catch(() => null)
      if (!doc) continue
      const { perms, grants } = await effectiveDocPerms(doc)
      if (!canRead(perms, user.id, user.email ?? user.name ?? null, grants)) continue
      chips.push({
        id: doc.id,
        filename: doc.title || 'Untitled',
        mime: 'ref/kb-doc',
        size: 0,
        refType: 'kb-doc',
        content: clip(doc.body),
      })
    } else {
      const artifact = await getArtifact(ref.id).catch(() => null)
      if (!artifact) continue
      const grants = await listEditors('artifact', artifact.id)
      if (!canRead(guarded(artifact), user.id, user.email ?? user.name ?? null, grants)) continue
      chips.push({
        id: artifact.id,
        filename: artifact.title || 'Untitled',
        mime: 'ref/artifact',
        size: 0,
        refType: 'artifact',
        content: clip(artifactToMarkdown(artifact)),
      })
    }
  }
  return chips
}

/** The prompt block a message's ref chips contribute — used at send time and
 *  by every history rebuild. */
export function refBlocks(attachments: unknown): string {
  if (!Array.isArray(attachments)) return ''
  const refs = attachments.filter(
    (a): a is RefChip => !!a && typeof a === 'object' && 'refType' in (a as Record<string, unknown>) && !!(a as RefChip).content,
  )
  if (refs.length === 0) return ''
  return refs
    .map((r) => `\n\n--- Attached ${r.refType === 'kb-doc' ? 'knowledge doc' : 'artifact'}: "${r.filename}" (${r.refType} ${r.id}) ---\n${r.content}`)
    .join('')
}
