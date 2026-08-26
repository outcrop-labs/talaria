// The command router — a typed two-or-three-word dispatch tree, hand-rolled
// because the repo runs on zero dependencies and the bash `case` blocks it
// replaces were hand-rolled too. Every command is declared ONCE here (name,
// aliases, summary, usage, flags) and the help text, the unknown-flag errors
// and the did-you-mean suggestions are all rendered FROM that declaration —
// the bash era's help was a sed of the script's own comment header, which
// drifted; this cannot.

import type { Ctx } from './ctx'
import { CliError } from './ui'

// ── Declarations ─────────────────────────────────────────────────────────────

export type FlagSpec = {
  name: string
  short?: string
  kind: 'bool' | 'value'
  desc: string
  /** Default for value flags; absent bools are simply not set. */
  default?: string
}

export type PositionalSpec = {
  name: string
  required?: boolean
  /** Takes the rest of the argv (e.g. `box enter <name> [cmd…]`). */
  multiple?: boolean
  desc?: string
}

export type ParsedArgs = {
  positionals: string[]
  flags: Record<string, string | boolean>
}

export type Leaf = {
  kind: 'leaf'
  name: string
  aliases?: string[]
  summary: string
  /** One-line usage shown in help; defaults to the path + positionals. */
  usage?: string
  flags?: FlagSpec[]
  positionals?: PositionalSpec
  run: (ctx: Ctx, args: ParsedArgs) => number | Promise<number>
}

export type Group = {
  kind: 'group'
  name: string
  aliases?: string[]
  summary: string
  children: Node[]
}

export type Node = Group | Leaf

// ── Flag parsing ─────────────────────────────────────────────────────────────

/** Parse `--flag value` / `--flag=value` / `-f value` / bools / `--`.
 *  Errors NAME THE COMMAND — `unknown flag --branh` at a bare prompt tells
 *  nobody anything. */
export function parseArgs(argv: string[], flags: FlagSpec[], cmdPath: string): ParsedArgs {
  const byName = new Map<string, FlagSpec>()
  for (const f of flags) {
    byName.set(`--${f.name}`, f)
    if (f.short) byName.set(`-${f.short}`, f)
  }
  const out: ParsedArgs = { positionals: [], flags: {} }
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]!
    if (a === '--') {
      out.positionals.push(...argv.slice(i + 1))
      break
    }
    if (a.startsWith('-') && a.length > 1) {
      const eq = a.indexOf('=')
      const token = eq === -1 ? a : a.slice(0, eq)
      const spec = byName.get(token)
      if (!spec) throw new CliError(`${cmdPath}: unknown flag ${token}`)
      if (spec.kind === 'bool') {
        if (eq !== -1) throw new CliError(`${cmdPath}: ${token} takes no value`)
        out.flags[spec.name] = true
      } else if (eq !== -1) {
        out.flags[spec.name] = a.slice(eq + 1)
      } else {
        if (i + 1 >= argv.length) throw new CliError(`${cmdPath}: ${token} needs a value`)
        out.flags[spec.name] = argv[++i]!
      }
    } else {
      out.positionals.push(a)
    }
  }
  for (const f of flags) {
    if (f.kind === 'value' && f.default !== undefined && !(f.name in out.flags)) {
      out.flags[f.name] = f.default
    }
  }
  return out
}

// ── Help ─────────────────────────────────────────────────────────────────────

export function renderHelp(path: string[], node: Node): string {
  const title = path.join(' ')
  const lines: string[] = []
  const usage =
    node.kind === 'leaf'
      ? node.usage ?? `${title} ${node.positionals?.multiple ? `<${node.positionals.name}> [${node.positionals.name}…]` : node.positionals ? `<${node.positionals.name}>` : ''}`.trim()
      : `${title} <command>`
  lines.push(`Usage: ${usage}`)
  lines.push('')
  if (node.kind === 'group') {
    lines.push('Commands:')
    for (const c of node.children) {
      const names = [c.name, ...(c.aliases ?? [])].join(', ')
      lines.push(`  ${names.padEnd(28)} ${c.summary}`)
    }
  } else {
    lines.push(node.summary)
    if (node.flags?.length) {
      lines.push('')
      lines.push('Flags:')
      for (const f of node.flags) {
        const tok = f.short ? `-${f.short}, --${f.name}` : `--${f.name}`
        const takes = f.kind === 'value' ? ' <value>' : ''
        lines.push(`  ${`${tok}${takes}`.padEnd(28)} ${f.desc}`)
      }
    }
  }
  return `${lines.join('\n')}\n`
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

const editDistance = (a: string, b: string): number => {
  // Small strings, plain DP — no deps for a did-you-mean.
  const d = Array.from({ length: a.length + 1 }, (_, i) => [i, ...Array(b.length).fill(0)])
  for (let j = 0; j <= b.length; j++) d[0]![j] = j
  for (let i = 1; i <= a.length; i++) {
    for (let j = 1; j <= b.length; j++) {
      d[i]![j] = Math.min(d[i - 1]![j]! + 1, d[i]![j - 1]! + 1, d[i - 1]![j - 1]! + (a[i - 1] === b[j - 1] ? 0 : 1))
    }
  }
  return d[a.length]![b.length]!
}

function suggest(node: Group, token: string): string | undefined {
  let best: string | undefined
  let bestD = 3 // beyond this a "suggestion" is noise
  for (const c of node.children) {
    for (const n of [c.name, ...(c.aliases ?? [])]) {
      const d = editDistance(token, n)
      if (d < bestD) {
        bestD = d
        best = c.name
      }
    }
  }
  return best
}

/** Walk the tree, handle --help at every level, parse flags at the leaf, run.
 *  CliError from anywhere prints once, here, and becomes exit code 1. */
export async function dispatch(ctx: Ctx, root: Group, argv: string[]): Promise<number> {
  try {
    return await dispatchInner(ctx, root, argv)
  } catch (e) {
    if (e instanceof CliError) {
      ctx.log.fail(e.message)
      return 1
    }
    throw e
  }
}

async function dispatchInner(ctx: Ctx, root: Group, argv: string[]): Promise<number> {
  let node: Node = root
  const path = [root.name]
  let i = 0
  for (; i < argv.length; i++) {
    const a = argv[i]!
    if (a === '--help' || a === '-h') {
      ctx.log.raw(renderHelp(path, node))
      return 0
    }
    if (a.startsWith('-') && a.length > 1) break // flags begin; node must be the leaf
    if (node.kind === 'leaf') break // first positional
    const next: Node | undefined = node.children.find((c) => c.name === a || c.aliases?.includes(a))
    if (!next) {
      const hint = suggest(node, a)
      throw new CliError(`unknown command \`${path.join(' ')} ${a}\`${hint ? ` — did you mean \`${hint}\`?` : ''}`)
    }
    node = next
    path.push(node.name)
  }
  if (node.kind === 'group') {
    if (i >= argv.length) {
      // Bare group: show the menu. (Bash printed the script header.)
      ctx.log.raw(renderHelp(path, node))
      return 0
    }
    if (argv[i]!.startsWith('-')) {
      throw new CliError(`${path.join(' ')}: unexpected flag ${argv[i]} — this is a command group; see \`${path.join(' ')} --help\``)
    }
    // Unreachable: unknown children threw inside the loop.
    throw new CliError(`unknown command \`${path.join(' ')} ${argv[i]}\``)
  }
  const rest = argv.slice(i)
  const args = parseArgs(rest, node.flags ?? [], path.join(' '))
  const spec = node.positionals
  if (spec) {
    if (spec.required && args.positionals.length === 0) {
      throw new CliError(`${path.join(' ')}: <${spec.name}> is required`)
    }
    if (!spec.multiple && args.positionals.length > 1) {
      throw new CliError(`${path.join(' ')}: unexpected argument \`${args.positionals[1]}\``)
    }
  } else if (args.positionals.length > 0) {
    throw new CliError(`${path.join(' ')}: unexpected argument \`${args.positionals[0]}\``)
  }
  return await node.run(ctx, args)
}
