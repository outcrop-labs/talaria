// Sparse (keyword) vectors for hybrid retrieval. Dense embeddings miss exact
// identifiers — ticket numbers, env vars, error strings, model names — so each
// chunk also gets a bag-of-terms vector: token → 32-bit hash index, value =
// saturated term frequency. Qdrant's sparse `modifier: idf` supplies the IDF
// half server-side at query time, so nothing here needs corpus statistics.

const STOPWORDS = new Set([
  'the', 'a', 'an', 'and', 'or', 'but', 'of', 'to', 'in', 'on', 'at', 'for', 'from', 'by', 'with', 'as',
  'is', 'are', 'was', 'were', 'be', 'been', 'being', 'it', 'its', 'this', 'that', 'these', 'those',
  'i', 'you', 'he', 'she', 'we', 'they', 'them', 'his', 'her', 'their', 'our', 'your', 'my', 'me', 'us',
  'not', 'no', 'so', 'if', 'then', 'than', 'too', 'very', 'can', 'will', 'just', 'do', 'does', 'did',
  'have', 'has', 'had', 'what', 'when', 'where', 'which', 'who', 'how', 'why', 'all', 'any', 'each',
  'there', 'here', 'about', 'into', 'over', 'under', 'again', 'also', 'up', 'down', 'out', 'off',
])

/** FNV-1a over the token — a stable u32 index. Collisions are rare enough to
 *  be noise in a scoring context. */
const fnv1a = (s: string): number => {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 0x01000193) >>> 0
  }
  return h
}

/** Tokens keep interior `.-_@/:` so identifiers survive whole:
 *  `TALARIA_EMBED_MODEL`, `bge-small-en-v1.5`, `sk-proj-…`, `PL-142`. */
function tokens(text: string): string[] {
  const out: string[] = []
  for (const raw of text.toLowerCase().split(/[^a-z0-9_@.\-/:]+/)) {
    const t = raw.replace(/^[.\-@/:]+|[.\-@/:]+$/g, '')
    if (t.length < 2 || STOPWORDS.has(t)) continue
    out.push(t)
    // An identifier also indexes under its parts, so "embed model" can meet
    // TALARIA_EMBED_MODEL halfway. The whole token stays the strongest signal.
    if (/[_.\-/:]/.test(t)) {
      for (const part of t.split(/[_.\-/:]+/)) {
        if (part.length >= 2 && !STOPWORDS.has(part)) out.push(part)
      }
    }
  }
  return out
}

export interface SparseVector {
  indices: number[]
  values: number[]
}

/** Encode text as a sparse vector: hashed terms with saturated tf (1 + ln n —
 *  a term's tenth repeat shouldn't count like its first). Empty text → empty
 *  vector (Qdrant accepts it; it simply matches nothing). */
export function sparseEncode(text: string): SparseVector {
  const tf = new Map<number, number>()
  for (const t of tokens(text)) {
    const h = fnv1a(t)
    tf.set(h, (tf.get(h) ?? 0) + 1)
  }
  const indices: number[] = []
  const values: number[] = []
  for (const [idx, n] of tf) {
    indices.push(idx)
    values.push(1 + Math.log(n))
  }
  return { indices, values }
}
