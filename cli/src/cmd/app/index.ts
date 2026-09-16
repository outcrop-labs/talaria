// `talaria app` — scaffold and work with Talaria apps (docs/APPS.md, docs/sdk/).
import type { Group } from '../../cli'
import { newCommand } from './new'

export const appCommand: Group = {
  kind: 'group',
  name: 'app',
  summary: 'scaffold Talaria apps (TypeScript against @talaria/sdk)',
  children: [newCommand],
}
