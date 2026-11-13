/** A skill's readable title, from its directory name: `load-matching` reads
 *  "Load matching". The name stays the identifier agents load it by. */
export function skillTitle(name: string): string {
  const title = name
    .split(/[-_\s]+/)
    .filter(Boolean)
    // A word with no vowel is an initialism (tms, pdf, sql), so it reads in capitals.
    .map((word) => (/^[b-df-hj-np-tv-xz]{2,}$/i.test(word) ? word.toUpperCase() : word.toLowerCase()))
    .join(' ')
  return title ? title[0]!.toUpperCase() + title.slice(1) : name
}
