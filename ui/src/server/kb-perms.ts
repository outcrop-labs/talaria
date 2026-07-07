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
export type GrantRole = 'viewer' | 'editor'

export interface EditorGrant {
  principalType: 'user' | 'agent'
  principalId: string
  role: GrantRole
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
    select principal_type as "principalType", principal_id as "principalId", role
    from kb_editors where item_type = ${itemType} and item_id = ${itemId}
  `) as unknown as EditorGrant[]
}

/** The set of item ids (of a type) a user has any grant on — for filtering
 *  lists (tree, folder list) so granted private items still show. */
export async function grantedItemIds(itemType: 'doc' | 'space', userId: string): Promise<Set<string>> {
  const sql = await db()
  const rows = (await sql`
    select item_id as "itemId" from kb_editors
    where item_type = ${itemType} and principal_type = 'user' and principal_id = ${userId}
  `) as unknown as Array<{ itemId: string }>
  return new Set(rows.map((r) => r.itemId))
}

export async function setEditors(itemType: 'doc' | 'space', itemId: string, grants: EditorGrant[]): Promise<void> {
  const sql = await db()
  await sql.begin(async (tx) => {
    await tx`delete from kb_editors where item_type = ${itemType} and item_id = ${itemId}`
    for (const g of grants) {
      await tx`
        insert into kb_editors (item_type, item_id, principal_type, principal_id, role)
        values (${itemType}, ${itemId}, ${g.principalType}, ${g.principalId}, ${g.role === 'editor' ? 'editor' : 'viewer'})
        on conflict (item_type, item_id, principal_type, principal_id)
        do update set role = excluded.role
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

/** Can this signed-in human read the item? Owner, org/public visibility, or any
 *  explicit grant (viewer or editor) on a private item. */
export function canRead(item: Guarded, userId: string | null, author: string | null, grants: EditorGrant[] = []): boolean {
  if (!userId) return false
  if (item.visibility !== 'private') return true // org or public → any member
  if (isOwner(item, userId, author)) return true
  return grants.some((g) => g.principalType === 'user' && g.principalId === userId)
}

/** Can this signed-in human edit the item? Owner, an org-wide edit policy (any
 *  reader edits), or an explicit *editor* grant. A viewer grant is read-only. */
export function canEditHuman(item: Guarded, userId: string | null, author: string | null, grants: EditorGrant[]): boolean {
  if (!userId) return false
  if (isOwner(item, userId, author)) return true
  if (item.editPolicy === 'org' && canRead(item, userId, author, grants)) return true
  return grants.some((g) => g.principalType === 'user' && g.principalId === userId && g.role === 'editor')
}

/** Can this agent (by model) edit the item? Only via an explicit editor grant. */
export function canEditAgent(agentModel: string, grants: EditorGrant[]): boolean {
  return grants.some((g) => g.principalType === 'agent' && g.principalId === agentModel && g.role === 'editor')
}
