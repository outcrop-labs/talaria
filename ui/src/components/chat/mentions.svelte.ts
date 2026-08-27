/** A composer mention option: `insert` is the token typed into the message. */
export interface Mentionable {
  insert: string
  label: string
  sub?: string
}

/** The composer token for @mentioning a user — mirrors the server's
 *  mention tokens (email localpart, else dashed full name). */
export const userMentionInsert = (u: { name: string | null; email: string | null }): string =>
  u.email?.split('@')[0] ?? (u.name ?? '').toLowerCase().replace(/\s+/g, '-')
