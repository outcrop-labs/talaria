// Comment threads on knowledge docs — Notion-shaped: a root comment may
// anchor to a QUOTE from the doc, replies thread under it, threads resolve.
// Access rides the doc's effective permissions (space-inherited + grants):
// anyone who can READ a doc can discuss it. Participants get notified.
import { db } from './db/pg'
import { getDoc, effectiveDocPerms } from './kb'
import { canRead } from './kb-perms'
import { addNotification } from './notifications'

export interface KbComment {
  id: string
  docId: string
  parentId: string | null
  authorUserId: string | null
  author: string
  quote: string | null
  content: string
  resolved: boolean
  createdAt: string
}

const ROW = `id, doc_id as "docId", parent_id as "parentId", author_user_id as "authorUserId",
  author, quote, content, resolved, created_at as "createdAt"`

/** Gate: the viewer can read the doc (comments are part of the doc). */
export async function canDiscussDoc(docId: string, userId: string, who: string | null): Promise<boolean> {
  const doc = await getDoc(docId)
  if (!doc) return false
  const { perms, grants } = await effectiveDocPerms(doc)
  return canRead(perms, userId, who, grants)
}

export async function listComments(docId: string): Promise<KbComment[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${ROW} from kb_comments where doc_id = $1 order by created_at asc`, [
    docId,
  ])) as unknown as KbComment[]
}

export async function addComment(input: {
  docId: string
  parentId: string | null
  authorUserId: string
  author: string
  quote: string | null
  content: string
}): Promise<KbComment> {
  const sql = await db()
  const rows = (await sql`
    insert into kb_comments (doc_id, parent_id, author_user_id, author, quote, content)
    values (${input.docId}, ${input.parentId}, ${input.authorUserId}, ${input.author}, ${input.quote}, ${input.content})
    returning ${sql.unsafe(ROW)}
  `) as unknown as KbComment[]
  const comment = rows[0]!

  // Notify the doc owner + everyone already in the thread (never the author).
  void (async () => {
    const doc = await getDoc(input.docId)
    if (!doc) return
    const targets = new Set<string>()
    if (doc.ownerUserId) targets.add(doc.ownerUserId)
    if (input.parentId) {
      const thread = (await sql`
        select distinct author_user_id as id from kb_comments
        where (id = ${input.parentId} or parent_id = ${input.parentId}) and author_user_id is not null
      `) as unknown as Array<{ id: string }>
      for (const t of thread) targets.add(t.id)
    }
    targets.delete(input.authorUserId)
    for (const userId of targets) {
      await addNotification(userId, {
        kind: 'kb-comment',
        title: `${input.author} commented on “${doc.title}”`,
        body: input.content.slice(0, 140),
        href: `/knowledge?d=${input.docId}`,
      }).catch(() => {})
    }
  })().catch(() => {})

  return comment
}

/** Resolve/unresolve a whole thread (root id). Author, thread starter, or the
 *  doc owner may do it. */
export async function setResolved(commentId: string, resolved: boolean, userId: string): Promise<boolean> {
  const sql = await db()
  const [c] = (await sql`
    select doc_id as "docId", author_user_id as "authorUserId", parent_id as "parentId" from kb_comments where id = ${commentId}
  `) as unknown as Array<{ docId: string; authorUserId: string | null; parentId: string | null }>
  if (!c) return false
  const rootId = c.parentId ?? commentId
  const doc = await getDoc(c.docId)
  const [root] = (await sql`select author_user_id as "authorUserId" from kb_comments where id = ${rootId}`) as unknown as Array<{
    authorUserId: string | null
  }>
  const may = c.authorUserId === userId || root?.authorUserId === userId || doc?.ownerUserId === userId
  if (!may) return false
  await sql`update kb_comments set resolved = ${resolved} where id = ${rootId} or parent_id = ${rootId}`
  return true
}

/** Delete own comment (its replies cascade). */
export async function deleteComment(commentId: string, userId: string): Promise<boolean> {
  const sql = await db()
  const rows = await sql`delete from kb_comments where id = ${commentId} and author_user_id = ${userId} returning 1 as ok`
  return rows.length > 0
}
