// Knowledgebase permissions — shared by docs and folders (spaces).
//
//   visibility  (read):  private → owner only · org → members · public → link
//   edit_policy (write):  owner → owner only · org → any reader · restricted →
//                         owner + an explicit editor list (users and/or agents)
//
// Agents never get implicit edit rights: even under 'org' they must be named in
// the editor list. That keeps automated edits deliberate.
import { db } from './db/pg'

export type Visibility = 'private' | 'org' | 'public'
export type EditPolicy = 'owner' | 'org' | 'restricted'

export interface EditorGrant {
  principalType: 'user' | 'agent'
  principalId: string
}

/** Shared shape both docs and spaces expose for permission checks. */
export interface Guarded {
  ownerUserId: string | null
  createdBy: string | null
  visibility: Visibility
  editPolicy: EditPolicy
}

export async function listEditors(itemType: 'doc' | 'space', itemId: string): Promise<EditorGrant[]> {
  const sql = await db()
  return (await sql`
    select principal_type as "principalType", principal_id as "principalId"
    from kb_editors where item_type = ${itemType} and item_id = ${itemId}
  `) as unknown as EditorGrant[]
}

export async function setEditors(itemType: 'doc' | 'space', itemId: string, grants: EditorGrant[]): Promise<void> {
  const sql = await db()
  await sql.begin(async (tx) => {
    await tx`delete from kb_editors where item_type = ${itemType} and item_id = ${itemId}`
    for (const g of grants) {
      await tx`
        insert into kb_editors (item_type, item_id, principal_type, principal_id)
        values (${itemType}, ${itemId}, ${g.principalType}, ${g.principalId})
        on conflict do nothing
      `
    }
  })
}

/** True if the human (by user id / author string) owns the item. Falls back to
 *  the author string for items created before owner ids were tracked. Only the
 *  owner may re-share (change visibility / edit policy / editor list). */
export function isOwner(item: Guarded, userId: string | null, author: string | null): boolean {
  if (item.ownerUserId) return !!userId && item.ownerUserId === userId
  return !!author && item.createdBy === author
}

/** Can this signed-in human read the item? */
export function canRead(item: Guarded, userId: string | null, author: string | null): boolean {
  if (!userId) return false
  if (item.visibility === 'private') return isOwner(item, userId, author)
  return true // org or public → any member
}

/** Can this signed-in human edit the item? */
export function canEditHuman(item: Guarded, userId: string | null, author: string | null, editors: EditorGrant[]): boolean {
  if (!userId) return false
  if (isOwner(item, userId, author)) return true
  if (item.editPolicy === 'owner') return false
  if (item.editPolicy === 'org') return canRead(item, userId, author)
  return editors.some((e) => e.principalType === 'user' && e.principalId === userId)
}

/** Can this agent (by model) edit the item? Only via an explicit grant. */
export function canEditAgent(agentModel: string, editors: EditorGrant[]): boolean {
  return editors.some((e) => e.principalType === 'agent' && e.principalId === agentModel)
}
