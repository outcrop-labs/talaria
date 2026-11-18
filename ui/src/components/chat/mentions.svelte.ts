/** A composer mention option: `insert` is the token typed into the message. */
export interface Mentionable {
  insert: string
  label: string
  sub?: string
}

/** The mention-token grammar, one rule on both sides: lowercase, every char
 *  outside [a-z0-9] mapped to '-', dash runs collapsed, edges trimmed. The
 *  server's content scanner (talaria-notify threads.rs mention_tokens) stops
 *  a token at any other char, so a raw dotted localpart like '@j.siegel'
 *  scanned to 'j' and never resolved (TALA-93). */
const normalizeToken = (s: string): string =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+/, '')
    .replace(/-+$/, '')

/** The composer token for @mentioning a user — mirrors the server's
 *  mention tokens (email localpart, else dashed full name), both spoken in
 *  the scanner's grammar. */
export const userMentionInsert = (u: { name: string | null; email: string | null }): string =>
  u.email ? normalizeToken(u.email.split('@')[0] ?? '') : u.name ? normalizeToken(u.name) : ''
