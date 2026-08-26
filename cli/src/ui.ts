// Console output — the ANSI parity of the bash scripts it replaces (▸ say,
// ✓ ok, ⚠ warn, ✗ die), with a NO_COLOR / non-TTY gate so agent-driven runs
// (no tty, NO_COLOR set) get plain text. Nothing here knows about commands.

export class CliError extends Error {}

/** Everything a command may print. `die` THROWS CliError; the dispatcher is
 *  the single place that turns a thrown CliError into a red line and exit
 *  code 1, so tests assert on the throw and the terminal sees the message
 *  exactly once. */
export type Log = {
  say(msg: string): void
  ok(msg: string): void
  skip(msg: string): void
  warn(msg: string): void
  die(msg: string): never
  /** The red ✗ line, without throwing — the dispatcher's error printer. */
  fail(msg: string): void
  /** Unformatted stdout (help text, `ls` tables). */
  raw(msg: string): void
}

const enabled = (isTTY: boolean): boolean => isTTY && !process.env.NO_COLOR

export function makeLog(isTTY: boolean): Log {
  const c = enabled(isTTY)
  const wrap = (code: string, s: string) => (c ? `\x1b[${code}m${s}\x1b[0m` : s)
  return {
    say: (m) => process.stdout.write(`${wrap('1;36', `▸ ${m}`)}\n`),
    ok: (m) => process.stdout.write(`  ${wrap('32', '✓')} ${m}\n`),
    skip: (m) => process.stdout.write(`  ${wrap('2', `– ${m}`)}\n`),
    warn: (m) => process.stderr.write(`  ${wrap('33', `⚠ ${m}`)}\n`),
    die: (m) => {
      throw new CliError(m)
    },
    fail: (m) => process.stderr.write(`${wrap('31', `✗ ${m}`)}\n`),
    raw: (m) => process.stdout.write(m),
  }
}
