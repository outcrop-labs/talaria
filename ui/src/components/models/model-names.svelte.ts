import { useModels, type GatewayModel } from '@/lib/muse.svelte'

/** A model id as the catalog spells it without its endpoint: `Anthropic/claude-x`
 *  is `claude-x`, and a bare id (which may hold a `/` of its own) is itself. */
export function bareModelId(m: Pick<GatewayModel, 'id' | 'endpoints' | 'qualified'>): string {
  if (!m.qualified) return m.id
  const endpoint = m.endpoints.find((e) => m.id.startsWith(`${e}/`))
  return m.id.slice((endpoint ? endpoint.length : m.id.indexOf('/')) + 1)
}

/** The catalog's human names, keyed by both spellings of each model's id. */
export function modelNameIndex(models: GatewayModel[]): Map<string, string> {
  const names = new Map<string, string>()
  for (const m of models) {
    if (!m.label || m.label === m.id) continue
    names.set(m.id, m.label)
    const bare = bareModelId(m)
    if (!names.has(bare)) names.set(bare, m.label)
  }
  return names
}

/** Looks up a model's human name, so a page leads with "Claude Sonnet 5.5"
 *  and keeps the id as quiet meta. Undefined when the catalog has no name. */
export function useModelNames(): (id: string | null | undefined) => string | undefined {
  const query = useModels()
  const names = $derived(modelNameIndex(query.data?.models ?? []))
  return (id) => (id ? names.get(id) : undefined)
}
