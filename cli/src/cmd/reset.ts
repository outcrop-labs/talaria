// `talaria reset` — the way back when something is wedged. Port of
// scripts/reset.sh.
//
// WHY THIS EXISTS — two failure modes have no in-app recovery, because the app
// cannot fix either of them from the inside:
//
//   · the encryption root is gone. Every provider key, agent secret and OAuth
//     token in the database is sealed with a key derived from
//     TALARIA_SECRET_KEY. Lose that value and the ciphertext is unrecoverable —
//     not by us, not by anyone. The app is right to refuse; what it cannot do
//     is decide FOR you that losing those secrets is acceptable. That is the
//     `secrets` mode: accept the loss, keep everything else, get a working
//     instance back.
//
//   · the database is in a state you no longer want to reason about.
//     Mid-migration, half-seeded, an experiment that went sideways. `database`
//     mode drops it and lets the next boot rebuild from the migration array.
//
// Neither is reversible, so both print exactly what they will destroy — with
// live counts from the actual database — and require you to type the mode name
// to proceed. There is no -y flag on purpose.
//
// TRY ADMIN → SECRETS FIRST. If the app starts at all, that page reports each
// secret's health individually and clears only the ones that are actually
// unreadable — an instance whose Google token predates a key change but whose
// provider key was entered yesterday keeps the second. `secrets` mode below
// cannot make that distinction and destroys everything sealed. This command is
// the backstop for when the app will not come up.
//
// WHICH STACK. Every target comes from this checkout's own ui/.env (stack.ts):
// inside a worktree that is the worktree's postgres container, compose project
// and fleet project, never main's defaults. It used to exec into
// `TALARIA_PG_CONTAINER ?? 'talaria-postgres-dev'` and sweep containers named
// `talaria-fleet-agent*`, so a reset run in a worktree reset MAIN. Before the
// confirm, the plan names every target. Anything ambiguous (a shell var naming
// another stack's container, a postgres that is not DATABASE_URL's, a rendered
// fleet file naming another project, the shim run from another checkout) dies
// before the first exec that changes anything.

import { existsSync, readFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import { compose } from '../compose'
import { envFileText, envValue } from '../envfile'
import { composeFileProject, describeStack, devStack, projectContainers, refuseForeignCheckout, verifyPostgres } from '../stack'

export type ResetMode = 'secrets' | 'database' | 'fleet'

/** Is this DATABASE_URL obviously a local dev database? The reset destroys
 *  data by design and has no undo; a typo in DATABASE_URL must not be able to
 *  take out something real. */
export function isLocalDb(url: string): boolean {
  return /@(127\.0\.0\.1|localhost):/.test(url)
}

export async function runReset(ctx: Ctx, mode: ResetMode): Promise<number> {
  const root = ctx.root
  refuseForeignCheckout(ctx, `reset ${mode}`)
  if (!existsSync(join(root, 'ui/.env'))) {
    ctx.log.die('ui/.env missing — nothing to reset (run `bun talaria setup` first)')
  }
  const uiEnv = envFileText(ctx, 'ui/.env')
  const dbUrl = envValue(uiEnv, 'DATABASE_URL')
  if (!dbUrl) ctx.log.die('no DATABASE_URL in ui/.env')
  // The guard runs BEFORE anything else touches the database.
  if (!isLocalDb(dbUrl)) {
    ctx.log.die('DATABASE_URL does not point at localhost — refusing. This command is for dev environments.')
  }

  // The stack is resolved from ui/.env, then the container proven to be the
  // database DATABASE_URL names (its compose project, its published port),
  // all before the first count runs.
  const stack = devStack(ctx, uiEnv)
  const pgc = stack.pgContainer
  const pgPort = await verifyPostgres(ctx, stack, dbUrl)

  // Counts run through exec WITHOUT stdin (the bash `docker exec` no-`-i`
  // rule): each one must not be able to swallow the line the operator is
  // about to type at the confirm prompt. execFile attaches no stdin, so the
  // trap cannot exist here — the rule survives as this comment.
  const psql = async (sql: string): Promise<string> => {
    try {
      return (await ctx.exec('docker', ['exec', pgc, 'psql', '-U', 'talaria', '-d', 'talaria', '-tAc', sql])).stdout.trim()
    } catch {
      return '0'
    }
  }
  // The destructive batches: SQL as an argv (never stdin), ON_ERROR_STOP so a
  // mid-batch failure aborts the transaction rather than half-applying. One
  // `-c` string runs as ONE transaction (psql sends it as a single request),
  // which is what the secrets clear wants. A list is one `-c` per statement,
  // each its own transaction, for statements that refuse to run inside one.
  const psqlBatch = (db: string, sql: string | string[]): Promise<number> =>
    ctx.run('docker', [
      'exec', pgc, 'psql', '-U', 'talaria', '-d', db, '-v', 'ON_ERROR_STOP=1',
      ...(Array.isArray(sql) ? sql : [sql]).flatMap((s) => ['-c', s]),
    ])

  // One line each: raw() writes no newline of its own, and without this every
  // count printed on one line.
  const row = (label: string, count: string | number): string => `  ${label.padEnd(42)} ${count}\n`
  // The plan: what this run acts on, named before the counts and the confirm,
  // so a wrong target is visible before anything can change.
  const target = (extra: [string, string][] = []): void => {
    ctx.log.say(`Target — ${describeStack(stack)}`)
    ctx.log.raw(row('postgres container', `${pgc} (compose project ${stack.infraProject}, 127.0.0.1:${pgPort})`))
    for (const [k, v] of extra) ctx.log.raw(row(k, v))
  }
  const confirm = async (word: ResetMode): Promise<void> => {
    const reply = await ctx.readLine(`\nThis cannot be undone. Type ${word} to proceed (anything else aborts): `)
    if (reply.trim() !== word) ctx.log.die('aborted — nothing was changed')
  }

  if (mode === 'secrets') {
    // Everything sealed with the data key, so the next boot mints a fresh one
    // and the app works again. What is cleared is the CIPHERTEXT, not the
    // feature: you re-enter provider keys and reconnect accounts, and keep
    // every board, ticket, document and conversation.
    target([['database', 'talaria']])
    ctx.log.say('What this will clear (live counts)')
    ctx.log.raw(row('llm_endpoints (provider API keys)', await psql("select count(*) from llm_endpoints where api_key_cipher is not null")))
    ctx.log.raw(row('agent_secrets (per-agent env secrets)', await psql('select count(*) from agent_secrets')))
    ctx.log.raw(row('agent_keys (per-agent credentials)', await psql('select count(*) from agent_keys')))
    ctx.log.raw(row('google_connections (per-user OAuth)', await psql('select count(*) from google_connections')))
    ctx.log.raw(row('google_org_connection (org OAuth)', await psql('select count(*) from google_org_connection')))
    ctx.log.raw(row('mcp_oauth_tokens (connected accounts)', await psql('select count(*) from mcp_oauth_tokens')))
    ctx.log.raw(row('mcp_user_credentials (headers)', await psql('select count(*) from mcp_user_credentials')))
    ctx.log.raw(row('app_settings: email/storage/github', await psql("select count(*) from app_settings where key in ('email_config','storage_config','github_config')")))
    ctx.log.raw(row('secret_keys (the data keys themselves)', await psql('select count(*) from secret_keys')) + '\n')
    ctx.log.say('What this will KEEP')
    for (const t of ['boards', 'tasks', 'users', 'agent_defs', 'messages']) {
      ctx.log.raw(row(t, await psql(`select count(*) from ${t}`)))
    }
    ctx.log.warn('Agents keep their identities and configuration; they lose their CREDENTIALS.')
    ctx.log.warn('After this: re-add provider keys on /models, reconnect Google and MCP accounts,')
    ctx.log.warn('then re-render the fleet so agents get fresh keys (Admin → Agents, or /api/fleet/render).')
    await confirm('secrets')

    ctx.log.say('Clearing')
    // Ciphertext columns first, then the keys themselves — so a crash midway
    // leaves unreadable-but-present rows rather than readable rows with no key.
    const code = await psqlBatch(
      'talaria',
      `begin;
update llm_endpoints set api_key_cipher = null;
delete from agent_secrets;
delete from agent_keys;
delete from google_connections;
delete from google_org_connection;
delete from mcp_oauth_tokens;
delete from mcp_user_credentials;
delete from app_settings where key in ('email_config','storage_config','github_config');
delete from secret_keys;
commit;`,
    )
    if (code !== 0) ctx.log.die('the clear batch failed — see the SQL error above; the transaction aborted, nothing was half-applied')
    ctx.log.ok('sealed data cleared — the next boot mints a fresh data key')
    ctx.log.warn('If you still have the ORIGINAL TALARIA_SECRET_KEY somewhere, stop and use it instead:')
    ctx.log.warn('restoring it recovers everything above. This command is the path when it is genuinely gone.')
    return 0
  }

  if (mode === 'database') {
    target([['database', 'talaria (dropped and recreated)']])
    ctx.log.say('What this will destroy — the ENTIRE database')
    for (const t of ['boards', 'tasks', 'users', 'agent_defs', 'messages', 'channels', 'kb_docs', 'artifacts']) {
      ctx.log.raw(row(t, await psql(`select count(*) from ${t}`)))
    }
    ctx.log.warn('Everything. The next boot replays the migration array into an empty database.')
    ctx.log.warn('Uploads on disk or in object storage are NOT touched — only Postgres.')
    await confirm('database')

    ctx.log.say('Dropping and recreating')
    // Three `-c`s, not one string: DROP DATABASE cannot run inside a
    // transaction block, and a single `-c` holding all three is one. That is
    // how this mode failed every time with "DROP DATABASE cannot run inside a
    // transaction block", after it had already terminated every connection.
    // WITH (FORCE) also ends a connection the app's pool opens in between.
    const code = await psqlBatch('postgres', [
      "select pg_terminate_backend(pid) from pg_stat_activity where datname = 'talaria' and pid <> pg_backend_pid()",
      'drop database if exists talaria with (force)',
      'create database talaria owner talaria',
    ])
    if (code !== 0) ctx.log.die('drop/recreate failed — see the SQL error above')
    ctx.log.ok('database recreated — restart the app and it will migrate from scratch')
    return 0
  }

  // fleet — the stack's own compose project, by label. The rendered file's
  // `name:` must agree with it: a file naming another project (a worktree
  // rendered before it had TALARIA_FLEET_PROJECT) means its agents may live
  // in that project, and which ones are this stack's is a guess.
  if (!stack.fleetProject) ctx.log.die(stack.fleetProblem ?? 'this stack has no fleet project to reset')
  const project = stack.fleetProject
  const composeFile = join(root, 'fleet/docker-compose.yml')
  const rendered = existsSync(composeFile) ? composeFileProject(readFileSync(composeFile, 'utf8')) : null
  if (rendered !== null && rendered !== project) {
    ctx.log.die(
      `fleet/docker-compose.yml names compose project '${rendered}', but this stack's fleet project is '${project}'. ` +
        'Its agents may be running under the other one, so refusing rather than guess. Re-render the fleet from this stack first.',
    )
  }
  const running = await projectContainers(ctx, project)
  const all = await projectContainers(ctx, project, true)
  target([['fleet compose project', project]])
  ctx.log.say('What this will remove')
  ctx.log.raw(row(`agent containers (project ${project})`, `${running.length} running, ${all.length} in all`))
  for (const n of all) ctx.log.raw(`    ${n}\n`)
  ctx.log.raw(row('fleet/ (rendered config + keys)', existsSync(join(root, 'fleet')) ? 'present' : 'absent'))
  ctx.log.warn('agent_defs in the database are KEPT — this removes only what the renderer produced.')
  ctx.log.warn('Re-render afterwards (Admin → Agents) to bring the fleet back.')
  await confirm('fleet')

  ctx.log.say('Removing')
  if (existsSync(composeFile)) {
    // -p names the project outright, so the down cannot follow anything but
    // this stack's; --env-file as the api passes it, so interpolation is the
    // render's own.
    const envFile = join(root, 'fleet/.env')
    await compose(ctx, { files: [composeFile], project, envFile: existsSync(envFile) ? envFile : undefined }, ['down', '--remove-orphans']).catch(() => {})
  }
  // A docker that refuses to list is not a reason to keep the rendered files:
  // projectContainers answers [] instead of throwing. The leftovers are read
  // again, by label, after the down.
  for (const n of await projectContainers(ctx, project, true)) {
    await ctx.exec('docker', ['rm', '-f', n]).catch(() => {})
  }
  // fleet/.env holds the agent credentials; it is regenerated by the renderer.
  for (const p of ['fleet/docker-compose.yml', 'fleet/fleet.json', 'fleet/.env', 'fleet/agents']) {
    rmSync(join(root, p), { recursive: true, force: true })
  }
  ctx.log.ok('fleet removed — re-render to rebuild it')
  return 0
}

export const resetCommand: Leaf = {
  kind: 'leaf',
  name: 'reset',
  summary: 'destructive resets for a wedged dev stack (typed confirm, no -y by design)',
  usage: 'talaria reset <secrets|database|fleet>',
  positionals: { name: 'mode', required: true, desc: 'secrets | database | fleet' },
  run: (ctx, args) => {
    const mode = args.positionals[0] as ResetMode
    if (mode !== 'secrets' && mode !== 'database' && mode !== 'fleet') {
      ctx.log.die(`unknown reset mode \`${mode}\` — secrets | database | fleet`)
    }
    return runReset(ctx, mode)
  },
}
