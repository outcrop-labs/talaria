// The composer's "/" command menu — its pure half: what the menu offers for a
// query, and what the agent actually receives when a command is sent.
//
// This module stays free of tiptap and of any component, because both halves
// of it are rules rather than plumbing and both have to be right without a
// browser to try them in. The imperative half — the Suggestion extension and
// the mounted CommandMenu — lives in ChatComposer.svelte beside the "@" and
// ":" menus it is a sibling of.
//
// Two sources, one list:
//
//   - the SHIPPED skills (`platform === true` in the skill library), which an
//     agent already knows how to run. Picking one inserts "/name " and the
//     send path turns it into a sentence (see `expandSlashCommand`).
//   - the viewer's BLUEPRINTS (lib/blueprints.ts) — text they kept to send
//     again. Picking one replaces the draft with its body and sends nothing.
//
// The menu's own last row, "Save as blueprint", is the other direction: the
// draft in the well becomes a blueprint. It is offered only when there is a
// draft to save, which is what `saveCommand` decides.
import { blueprintSummary, canDeleteBlueprint, type Blueprint } from '@/lib/blueprints'
import type { SkillOwner } from '@/lib/skills'

/** A command-able skill, flattened out of the library read. */
export interface CommandSkill {
  name: string
  description: string
}

export type CommandKind = 'skill' | 'blueprint' | 'save'

export interface CommandItem {
  kind: CommandKind
  /** Row identity; for a skill, the token it inserts. */
  key: string
  label: string
  /** The muted second column: a skill's description, a blueprint's first line. */
  detail: string
  /** blueprint: the body that replaces the draft. save: the body to store. */
  body?: string
  /** blueprint only. */
  id?: string
  /** blueprint only — the viewer owns it, so the row offers a delete. */
  canDelete?: boolean
}

/** The shipped set, de-duplicated: the shared root and every agent list the
 *  same platform skills, and this menu is a list of commands, not of owners.
 *  Sorted by name so a tie in the ranking below never reshuffles between
 *  keystrokes. */
export function platformSkills(owners: readonly SkillOwner[]): CommandSkill[] {
  const byName = new Map<string, CommandSkill>()
  for (const owner of owners) {
    for (const skill of owner.skills ?? []) {
      if (skill.platform !== true) continue
      if (!byName.has(skill.name)) byName.set(skill.name, { name: skill.name, description: skill.description ?? '' })
    }
  }
  return [...byName.values()].sort((a, b) => a.name.localeCompare(b.name))
}

/** Rows the menu will show at once. The save row is appended after this cap,
 *  because it is the menu's own action and not a match. */
const MAX_MATCHES = 8

/** Prefix 0, substring 1, no match -1. An EMPTY query matches everything as a
 *  prefix, so "/" alone opens the whole menu in its natural order. */
function rankOf(name: string, query: string): number {
  if (!query) return 0
  const n = name.toLowerCase()
  if (n.startsWith(query)) return 0
  return n.includes(query) ? 1 : -1
}

/** The ranked candidates for what has been typed after the "/".
 *
 *  Prefix matches before substring ones, skills before blueprints, then source
 *  order — a person typing a command they know is reaching for a skill, and a
 *  name they typed the start of is the one they meant. */
export function filterCommands(
  query: string,
  skills: readonly CommandSkill[],
  blueprints: readonly Blueprint[],
  opts: { viewerId?: string | null } = {},
): CommandItem[] {
  const q = query.trim().toLowerCase()
  const scored: { item: CommandItem; rank: number; group: number; at: number }[] = []
  skills.forEach((s, at) => {
    const rank = rankOf(s.name, q)
    if (rank < 0) return
    scored.push({ rank, group: 0, at, item: { kind: 'skill', key: s.name, label: s.name, detail: s.description } })
  })
  blueprints.forEach((b, at) => {
    const rank = rankOf(b.name, q)
    if (rank < 0) return
    scored.push({
      rank,
      group: 1,
      at,
      item: {
        kind: 'blueprint',
        key: b.id,
        label: b.name,
        detail: blueprintSummary(b.body),
        body: b.body,
        id: b.id,
        canDelete: canDeleteBlueprint(b, opts.viewerId ?? null),
      },
    })
  })
  scored.sort((a, b) => a.rank - b.rank || a.group - b.group || a.at - b.at)
  return scored.slice(0, MAX_MATCHES).map((s) => s.item)
}

/** The leading command token: "/name", where the name is followed by a space
 *  or the end of the text. The lookahead keeps the match to the token itself,
 *  so what follows survives verbatim. */
// Multiline: the menu opens at the start of ANY text block (a new paragraph
// or a hard break qualifies), so the command can sit on a later line and the
// expansion has to find it where the menu put it.
const LEADING_COMMAND = /^\/([A-Za-z0-9][A-Za-z0-9._-]*)(?=\s|$)/m

/** The whole draft IS the command token — "/gi" mid-type, or "/github" with
 *  nothing after it. There is no draft to keep from that. */
export function isBareCommand(text: string): boolean {
  return /^\/\S*$/.test(text.trim())
}

/** What a blueprint saved from the well would hold: the draft with the
 *  command-in-progress taken off the front. The "/" and its token are what
 *  opened the menu, not something the person wrote. */
export function blueprintBodyFrom(text: string): string {
  // The command-in-progress is a bare "/token" line: at the front of the
  // draft when the menu opened on the first line, at the END when it opened on
  // a new paragraph after the draft. Neither is something the person wrote.
  return text
    .trim()
    .replace(/^\/\S*[ \t]*/, '')
    .replace(/(?:^|\n)[ \t]*\/\S*[ \t]*$/, '')
    .trim()
}

/** Whether the menu's last row belongs on the menu at all. */
export function offersSave(text: string): boolean {
  return !isBareCommand(text) && blueprintBodyFrom(text).length > 0
}

/** The menu's last row, or nothing when there is no draft behind the "/". */
export function saveCommand(text: string): CommandItem | null {
  if (!offersSave(text)) return null
  return { kind: 'save', key: 'save', label: 'Save as blueprint', detail: 'Keep this draft to send again', body: blueprintBodyFrom(text) }
}

/** Everything the menu shows, in order: the ranked matches, then the save row
 *  when there is a draft to save. One call, so the row order the keyboard
 *  navigates is the row order the component renders. */
export function commandItems(args: {
  query: string
  skills: readonly CommandSkill[]
  blueprints: readonly Blueprint[]
  /** The composer's whole text, for the save row. */
  text: string
  viewerId?: string | null
}): CommandItem[] {
  const matches = filterCommands(args.query, args.skills, args.blueprints, { viewerId: args.viewerId })
  const save = saveCommand(args.text)
  return save ? [...matches, save] : matches
}

/** What the agent receives for a command, and what the transcript shows.
 *
 *  A slash command is a request to use a skill, so that is what gets SENT: the
 *  agent reads a message, not a menu, and a message is what the person is
 *  accountable for having said. "/github fix the CI" travels as "Use your
 *  `github` skill. fix the CI".
 *
 *  Only a LEADING token followed by a space or the end of the text counts, and
 *  only one the library actually has: a URL's slash, a date, a path, and an
 *  unknown command all travel untouched. The canonical name from the library
 *  is the one quoted, so "/GitHub" reaches the agent as the directory it has. */
export function expandSlashCommand(text: string, knownSkillNames: readonly string[]): string {
  const m = LEADING_COMMAND.exec(text)
  const typed = m?.[1]
  if (!m || !typed) return text
  const name =
    knownSkillNames.find((n) => n === typed) ?? knownSkillNames.find((n) => n.toLowerCase() === typed.toLowerCase())
  if (!name) return text
  const head = `Use your \`${name}\` skill.`
  // Whatever stood before the command line stays; the separator the person
  // typed after the command is theirs to keep: a newline after it stays a
  // newline, so a command above a pasted block reads as one.
  const before = text.slice(0, m.index)
  const rest = text.slice(m.index + m[0].length).replace(/^[ \t]+/, '')
  if (!rest) return `${before}${head}`
  return `${before}${/^\s/.test(rest) ? `${head}${rest}` : `${head} ${rest}`}`
}
