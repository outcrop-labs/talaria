// The command tree — one declaration per command, the single source help is
// rendered from. Commands register themselves here as they are ported; each
// port deletes its bash script in the same commit (docs/CHANGELOG follow).

import type { Group } from '../cli'

export const tree: Group = {
  kind: 'group',
  name: 'talaria',
  summary: 'Talaria — every way to drive the repo, from one place',
  children: [],
}
