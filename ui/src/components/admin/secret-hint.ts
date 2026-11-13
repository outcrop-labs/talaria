/** The one sentence under a field that takes a secret: why it has its own
 *  Save, and what happens to the one already there. `noun` is what the
 *  field holds: a key, a password, a token. */
export const secretHint = (stored: boolean, noun = 'key'): string =>
  stored
    ? `A ${noun} is stored. Enter a new one and save to replace it.`
    : `Stored encrypted when you save, and never shown again.`
