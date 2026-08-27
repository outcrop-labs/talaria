// Shared @mention handling across surfaces (channels, ticket comments, and —
// as they land — plans and research). A mention notifies any member the actor
// can reach; agent mentions are surfaced separately by each surface (channels
// trigger replies; comments pull the agent's attention).
import { addNotification } from './notifications'

/** Tokens a person answers to: email localpart, dashed full name, first name. */
function userMentionTokens(name: string | null, email: string | null): string[] {
  const tokens = new Set<string>()
  const local = email?.split('@')[0]?.toLowerCase()
  if (local) tokens.add(local)
  const n = name?.trim().toLowerCase()
  if (n) {
    tokens.add(n.replace(/\s+/g, '-'))
    tokens.add(n.split(/\s+/)[0]!)
  }
  return [...tokens]
}

/** The @tokens present in a body (lowercased, without the leading @). */
function mentionTokens(content: string): Set<string> {
  return new Set([...content.matchAll(/@([a-z0-9][a-z0-9-]*)/gi)].map((m) => m[1]!.toLowerCase()))
}

export interface Mentionee {
  userId: string
  name: string | null
  email: string | null
}

/** Notify every member the body @mentions (never the sender). Generic over the
 *  member set, so any surface with a membership list can reuse it. */
export async function notifyMentions(
  members: Mentionee[],
  senderUserId: string,
  senderLabel: string,
  content: string,
  where: string, // e.g. "#general" or a ticket ref
  href: string,
): Promise<void> {
  const mentions = mentionTokens(content)
  if (mentions.size === 0) return
  for (const m of members) {
    if (m.userId === senderUserId) continue
    if (!userMentionTokens(m.name, m.email).some((t) => mentions.has(t))) continue
    await addNotification(m.userId, {
      kind: 'mention',
      title: `${senderLabel} mentioned you in ${where}`,
      body: content.length > 200 ? `${content.slice(0, 200)}…` : content,
      href,
    }).catch(() => {})
  }
}
