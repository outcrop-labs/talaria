// Agent role templates client — the business roles a new agent starts from.
// Talaria maintains a built-in set; an org adds its own, and an org template
// with the same slug shadows the built-in of that name.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

export interface RoleTemplate {
  slug: string
  name: string
  role: string
  department: string
  description: string
  soul: string
  /** False for the org's own templates — the picker groups on this. */
  builtIn: boolean
}

export async function listRoleTemplates(): Promise<RoleTemplate[]> {
  const { templates } = await getJson<{ templates: RoleTemplate[] }>('/api/agent-role-templates')
  return templates
}

export const ROLE_TEMPLATES_KEY = ['agent-role-templates'] as const

/** The role library, read the way every other list in the app is read.
 *
 *  This used to be a `$state` + `$effect` fetch inside the view, with its own
 *  `loading` and `loadErr` strings. That is a second answer to a question the
 *  app already answers — and the hand-rolled one had no cache, so the list
 *  refetched from scratch every time the tab was opened, and no invalidation,
 *  so a save had to manually re-run the loader. */
export function useRoleTemplates() {
  return createQuery(() => ({
    queryKey: ROLE_TEMPLATES_KEY,
    queryFn: (): Promise<RoleTemplate[]> => listRoleTemplates(),
  }))
}

export async function saveRoleTemplate(t: Omit<RoleTemplate, 'builtIn'>): Promise<RoleTemplate> {
  const { template } = await getJson<{ template: RoleTemplate }>('/api/agent-role-templates', {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(t),
  })
  return template
}

export async function deleteRoleTemplate(slug: string): Promise<void> {
  await getJson<{ ok: true }>(`/api/agent-role-templates?slug=${encodeURIComponent(slug)}`, { method: 'DELETE' })
}
