// Who may tailor which skills. The Studio principle: you can shape the
// agents you've been explicitly granted, admins and agents.manage shape the
// fleet. "May use by default" (a member with NO user_agent_access rows) does
// NOT imply "may rewrite how it works" — tailoring rights are explicit.
import { hasPerm } from './permissions'
import { ownsAgent } from './personal-agent'
import { allowedAgents } from './users'
import { ownerModel, SHARED } from './agent-skills'

export async function canEditSkills(user: { id: string; role: 'admin' | 'member' }, owner: string): Promise<boolean> {
  if (user.role === 'admin') return true
  if (await hasPerm(user, 'agents.manage')) return true
  if (owner === SHARED) return false // fleet-wide flow changes need agents.manage
  if (await ownsAgent(user.id, { slug: owner })) return true // your personal assistant
  const model = await ownerModel(owner)
  if (!model) return false
  const access = await allowedAgents(user.id, user.role)
  // Only an EXPLICIT grant confers tailoring — 'all' is the unrestricted-use
  // default, not a statement of trust over every agent's behavior.
  return access !== 'all' && access.includes(model)
}
