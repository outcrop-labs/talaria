/** The one sentence under a field that takes a secret: why it has its own
 *  Save, and what happens to the one already there. `noun` is what the
 *  field holds: a key, a password, a token. */
export const secretHint = (stored: boolean, noun = 'key'): string =>
  stored
    ? `A ${noun} is stored, and saving a new one replaces it.`
    : `Stored encrypted when you save, and never shown again.`

/** The widths the secret inventory's header captions and each row's cells share. */
export const SECRET_COLS = { state: 'w-[9rem] shrink-0', set: 'w-[4.5rem] shrink-0' } as const
