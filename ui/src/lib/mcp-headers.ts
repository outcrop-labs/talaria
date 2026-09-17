// Header declarations → form fields. The MCP registry's `InputWithVariables`
// schema puts credentials in a `value` template ("Bearer {api_key}"), with an
// optional `variables` map describing each fill-in; a declaration with NO
// value means the person types the whole header; a value with no braces is a
// fixed header the publisher sets. These helpers turn declarations into the
// fields a form renders and compose the typed values back into final header
// values. Lives in lib/ (not routes/) so vitest can reach it.

export interface HeaderVariable {
  description: string | null
  isSecret: boolean
  placeholder: string | null
  default: string | null
  choices: string[] | null
}

export interface HeaderDecl {
  name: string
  description: string | null
  isRequired: boolean
  isSecret: boolean
  placeholder: string | null
  default: string | null
  choices: string[] | null
  value: string | null
  variables: Record<string, HeaderVariable> | null
}

/** One thing a person types: either a template variable inside a header's
 *  `value`, or a whole header value when the declaration has none. */
export interface HeaderField {
  /** Form key: the header's name, or `header::variable` for template vars. */
  key: string
  /** What the label shows: the variable's name, or the header's. */
  label: string
  /** The header this field ultimately feeds. */
  header: string
  isRequired: boolean
  isSecret: boolean
  placeholder: string | null
  description: string | null
  choices: string[] | null
  default: string | null
}

/** The `{variables}` named in a template, in order, deduped. Braces whose
 *  inside isn't `[A-Za-z0-9_-]+` (or is empty) stay literal — the registry
 *  schema says unresolved braces remain unchanged. */
export function templateVars(template: string): string[] {
  const vars: string[] = []
  let i = 0
  while (i < template.length) {
    if (template[i] === '{') {
      const end = template.indexOf('}', i + 1)
      if (end > i + 1) {
        const inner = template.slice(i + 1, end)
        if (isVarName(inner) && !vars.includes(inner)) vars.push(inner)
        i = end + 1
        continue
      }
    }
    i++
  }
  return vars
}

function isVarName(s: string): boolean {
  return /^[A-Za-z0-9_-]+$/.test(s)
}

/** The fields a form renders for one declaration: none for a fixed header,
 *  one per variable for a template, one whole-header field otherwise. */
export function headerFields(h: HeaderDecl): HeaderField[] {
  if (h.value == null) {
    return [
      {
        key: h.name,
        label: h.name,
        header: h.name,
        isRequired: h.isRequired,
        isSecret: h.isSecret,
        placeholder: h.placeholder,
        description: h.description,
        choices: h.choices,
        default: h.default,
      },
    ]
  }
  return templateVars(h.value).map((v) => {
    const meta = h.variables?.[v]
    return {
      key: `${h.name}::${v}`,
      label: v,
      header: h.name,
      isRequired: h.isRequired,
      isSecret: meta?.isSecret ?? h.isSecret,
      placeholder: meta?.placeholder ?? null,
      description: meta?.description ?? h.description,
      choices: meta?.choices ?? null,
      default: meta?.default ?? null,
    }
  })
}

/** The final header value from typed field values. Unresolved variables keep
 *  their braces — never guess. */
export function composeHeader(h: HeaderDecl, values: Record<string, string>): string {
  if (h.value == null) return values[h.name] ?? ''
  return h.value.replace(/\{([A-Za-z0-9_-]+)\}/g, (braces, name: string) => {
    const v = values[`${h.name}::${name}`]
    return v && v.trim() ? v : braces
  })
}

/** Fixed headers (value, no variables) — the publisher sets these, so they
 *  apply automatically and no form ever prompts for them. */
export function literalHeaders(decls: HeaderDecl[]): Record<string, string> {
  const out: Record<string, string> = {}
  for (const h of decls) {
    if (h.value != null && templateVars(h.value).length === 0) out[h.name] = h.value
  }
  return out
}

/** Composed headers from filled values: literals auto-applied, plus every
 *  declared header with at least one filled field. A header whose fields
 *  were all left empty is omitted — never store `Bearer {key}`. */
export function composeHeaders(decls: HeaderDecl[], values: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = literalHeaders(decls)
  for (const h of decls) {
    const fields = headerFields(h)
    if (fields.length === 0) continue
    if (!fields.some((f) => (values[f.key] ?? '').trim())) continue
    out[h.name] = composeHeader(h, values)
  }
  return out
}
