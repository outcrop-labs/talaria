// Agent role templates client — the business roles a new agent starts from.
// Talaria maintains a built-in set; an org adds its own, and an org template
// with the same slug shadows the built-in of that name.
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
