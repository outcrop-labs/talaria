export const PERSON_TINTS = 6

export function personTint(name: string): number {
  let hash = 0x811c9dc5
  for (const char of name.trim().toLowerCase().replace(/\s+/g, ' ')) {
    hash ^= char.codePointAt(0)!
    hash = Math.imul(hash, 0x01000193) >>> 0
  }
  return hash % PERSON_TINTS
}
