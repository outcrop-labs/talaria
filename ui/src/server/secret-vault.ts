// CREDENTIALS DO NOT GO INTO A MODEL'S CONTEXT.
//
// WHY THIS EXISTS ALONGSIDE THE GUARD, WHICH ALREADY CATCHES SECRETS. The guard
// is an OUTPUT check: `secret_leak` fires on what a model wrote, and
// `redactSecrets` cleans what gets persisted. Measured across the registry, that
// half is complete — all 27 harnesses declare the rule and 26 redact.
//
// It is also, by construction, too late. By the time `secret_leak` fires the
// model has already READ the credential: it can act on it, put it in a tool
// call, quote it into a document the guard does not cover, and the provider has
// already logged the prompt on its own infrastructure. Redaction cleans what we
// KEEP. It cannot clean what we SENT.
//
// The adversarial tier says how much that matters. On this install the four
// strongest models file `secret_leak` on two of four seeds each, AFTER grounding
// — which is the honest reading of "no model is perfect at this". A business
// asking whether its keys are safe here deserves an answer that does not depend
// on the model's judgement at all.
//
// SO THE VALUE NEVER TRAVELS. A credential in outbound context is replaced by an
// opaque HANDLE before the request leaves the process. The model sees
// `«secret:1»`, can reason about it, can pass it to a tool — and the platform
// substitutes the real value only at the boundary where it is actually used.
// What leaks, if anything leaks, is a handle that means nothing anywhere else.
//
// WHAT THIS IS NOT. It is not a vault product and does not store anything: a
// `SecretVault` lives for one request and is garbage when it returns. It does
// not stop a model reciting a credential from its own training data — that is
// what `secret_leak` on the output side is for, and the two halves are
// complementary rather than redundant.


/** THE CREDENTIAL SHAPES — one list, and it lives HERE rather than in
 *  guardrails.ts.
 *
 *  BOTH HALVES READ IT: this file seals them on the way out, `redactSecrets`
 *  cleans them out of what gets kept. A second copy would mean a credential the
 *  guard knows about travelling to a provider because the sealer had not heard
 *  of it.
 *
 *  AND THE DEPENDENCY POINTS THIS WAY DELIBERATELY. The sealer runs on the path
 *  every model call takes; its knowledge of what a credential IS must not be
 *  something a test can mock away or a refactor can leave behind. guardrails.ts
 *  imports from here, never the reverse — found the moment the patterns lived
 *  there and a partial `vi.mock('./guardrails')` silently disarmed the sealing
 *  for four tests. */
export const SECRET_PATTERNS: Array<{ label: string; re: RegExp; redactRe?: RegExp }> = [
  // Before the OpenAI rule: `sk-ant-…` also satisfies the looser `sk-…` shape,
  // and whichever matches first decides the label a human sees.
  { label: 'Anthropic key', re: /\bsk-ant-[A-Za-z0-9_-]{20,}\b/ },
  { label: 'OpenAI key', re: /\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}\b/ },
  // Stripe live + restricted-live keys. Underscore-separated, so neither `sk-`
  // rule above ever saw them. Test keys (sk_test_…) are deliberately excluded:
  // they appear in every tutorial and are not a live credential.
  { label: 'Stripe secret key', re: /\b[sr]k_live_[A-Za-z0-9]{16,}\b/ },
  { label: 'AWS access key', re: /\bAKIA[0-9A-Z]{16}\b/ },
  // Google keys are a fixed 39 characters; the exact count is the precision.
  { label: 'Google API key', re: /\bAIza[0-9A-Za-z_-]{35}\b/ },
  { label: 'Slack token', re: /\bxox[baprs]-[0-9A-Za-z-]{10,}\b/ },
  // Slack APP-level token (Socket Mode). Not an xox* shape, so it needs its own.
  { label: 'Slack app token', re: /\bxapp-[0-9A-Za-z-]{10,}\b/ },
  { label: 'GitHub token', re: /\bgh[pousr]_[A-Za-z0-9]{36,}\b/ },
  // Fine-grained PAT. `gh[pousr]_` cannot match `github_pat_`, and this is the
  // exact shape the workbench hands a dev agent in PAT mode — the credential an
  // agent is most likely to have in context and echo back.
  { label: 'GitHub fine-grained token', re: /\bgithub_pat_[A-Za-z0-9_]{30,}\b/ },
  { label: 'Talaria gateway key', re: /\btlk_[a-f0-9]{40,}\b/ },
  // The agent's OWN credential (`tak_` + 24 random bytes hex, agent-auth.ts).
  // Of every secret in this list it is the one an agent is most certain to
  // have in its environment, and the guard did not know the prefix existed.
  { label: 'Talaria agent credential', re: /\btak_[a-f0-9]{40,}\b/ },
  // user:password@host in any URI — how database, registry and proxy
  // credentials actually leak. Requires BOTH a userinfo colon and an '@', so
  // ordinary links (https://host:8443/path, ssh://user@host) don't match.
  { label: 'Credentials in URL', re: /\b[a-z][a-z0-9+.-]*:\/\/[^\s:/?#@]*:[^\s/?#@]+@[^\s/?#]+/i },
  {
    label: 'Private key block',
    // THE LINE BREAK AFTER THE HEADER IS LOAD-BEARING, and it is the difference
    // between a credential and a sentence ABOUT a credential. PEM always puts
    // the body on its own line; prose never does ("look for the -----BEGIN
    // PRIVATE KEY----- line in the bundle"). Without it the marker alone
    // matched, and because the redaction below swallows to end-of-text when the
    // block is unterminated, one such SENTENCE deleted everything after it —
    // then the distiller archived the chat, the librarian overwrote a good OKF,
    // and a work session lost the trailing DONE the dispatch loop parses.
    re: /-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----[ \t]*\r?\n/,
    // Redaction must swallow the whole block (or to end-of-text if unterminated,
    // which is how a key truncated mid-stream arrives), not just the BEGIN line.
    redactRe: /-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----[ \t]*\r?\n[\s\S]*?(?:-----END (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----|$)/,
  },
]

/** THE HANDLE SHAPE, and every part of it is deliberate.
 *
 *  GUILLEMETS because no credential format uses them, so a handle can never be
 *  confused for the thing it replaces, and a model that echoes one back is
 *  unambiguous rather than plausibly a real key.
 *
 *  A NUMBER, not the value's hash: a hash of a short credential is a brute-force
 *  target, and a hash is stable across requests, which would let two runs be
 *  correlated by the secrets they touched. The counter restarts per vault. */
const HANDLE = /«secret:(\d+)»/g
const handleFor = (n: number): string => `«secret:${n}»`

export interface SealedSecret {
  handle: string
  /** What KIND it is — the pattern's own label. Safe to log and to show a human;
   *  it names the shape, never the value. */
  label: string
}

/** One request's substitutions. Lives as long as the call and no longer — there
 *  is no store behind this, deliberately. */
export interface SecretVault {
  /** handle -> the real value. The only place it exists outside the caller. */
  values: Map<string, string>
  /** What was sealed, for the audit line. Labels only. */
  sealed: SealedSecret[]
}

export const newVault = (): SecretVault => ({ values: new Map(), sealed: [] })

/** Replace every credential in `text` with a handle, recording the substitution.
 *
 *  IDEMPOTENT AND STABLE WITHIN A VAULT: the same value seen twice — in the
 *  system prompt and again in a tool result — gets the SAME handle, so a model
 *  reading both sees one consistent thing rather than two mysteries. */
export function sealText(text: string, vault: SecretVault): string {
  let out = text
  for (const { label, re, redactRe } of SECRET_PATTERNS) {
    const base = redactRe ?? re
    const g = new RegExp(base.source, base.flags.includes('g') ? base.flags : `${base.flags}g`)
    out = out.replace(g, (match) => {
      for (const [handle, value] of vault.values) if (value === match) return handle
      const handle = handleFor(vault.values.size + 1)
      vault.values.set(handle, match)
      vault.sealed.push({ handle, label })
      return handle
    })
  }
  return out
}

/** Put the real values back. Called ONLY at the boundary where the value is
 *  used — an outbound tool call, a header, a credential helper — never on
 *  anything that goes back to a model or into a record.
 *
 *  A handle this vault does not know is left alone: it is either a model
 *  inventing one (which must not resolve to anything) or a handle from another
 *  request (which must not resolve here). Both are the same rule — a vault
 *  answers only for what it sealed. */
export function unsealText(text: string, vault: SecretVault): string {
  return text.replace(HANDLE, (match) => vault.values.get(match) ?? match)
}

/** Did the model hand back a handle it was never given? A model that invents
 *  `«secret:9»` is guessing at a credential it cannot see, which is worth
 *  knowing about even though the guess resolves to nothing. */
export function inventedHandles(text: string, vault: SecretVault): string[] {
  return [...new Set(text.match(HANDLE) ?? [])].filter((h) => !vault.values.has(h))
}

/** Seal a whole message list in place of the caller having to walk it. Returns
 *  the sealed copy — the input is never mutated, because the caller may still
 *  need the original for grounding (`redactSecrets` takes the turn's input, and
 *  handing it a sealed copy would ground nothing). */
export function sealMessages<T extends { content: string }>(messages: readonly T[], vault: SecretVault): T[] {
  return messages.map((m) => (typeof m.content === 'string' && m.content ? { ...m, content: sealText(m.content, vault) } : m))
}
