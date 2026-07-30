# workbench-driving

When to use: whenever you run coding work through your workbench — start_job, a coding harness (opencode, Claude Code, Codex, Oh My Pi), finish_job. This is the discipline that makes you a good driver, not a dispatcher.

## Before you build — be curious

1. Read the ticket like an engineer, not a courier. If the requirement is ambiguous — unclear scope, missing acceptance criteria, two plausible interpretations — **ask on the ticket first** (a comment with your specific question) instead of guessing. A sharp question early beats a wrong PR later.
2. Read the code you're about to change. Use get_ticket for context, clone, and look before planning. Your plan should mention real files, not hopes.
3. Know your harness. start_job returns a `guide` for your chosen harness — read it. Know how it reports results, how sessions resume, what it can and can't verify.

## While you build — drive, don't fling

4. One workspace per job: clone into the `workdir` start_job gives you and never work outside it — concurrent jobs stay isolated that way. Your harness's session history lives under `/opt/data/workbench/harness/`, persists across restarts, and is shared with your department: resume your own earlier sessions, or pick up a teammate's hand-off instead of starting cold.
5. Prefer the harness's **MCP tools** when they're registered on your config — you get structured tool results, not text to guess at. Otherwise use the `jsonRun` invocation and read the structured result object (result text, files touched, session id). Never scrape raw logs for meaning.
6. Iterate in conversation with the harness: run, read the structured result, ask follow-ups (resume the session where supported) until you understand what changed and why. If the harness's answer surprises you, dig — surprise is information. Your work session spans MANY turns — Talaria keeps the conversation going until the ticket reaches review or blocked, so never compress real work into one pass to "finish the reply".
7. Test UIs like a user, not a compiler: for anything with a front end, drive it in a real browser with Playwright (`npx playwright`) — load the page, click the flow, assert what a human would see, and screenshot the result as evidence for the ticket. A UI change without a browser check is unverified.
8. You own the result, not the harness. After it works: read your own diff (`git diff`), run the repo's tests or verify commands, and check the change does only what the ticket asked.

## Finishing

9. finish_job only when the branch holds work you have personally verified. Your summary should say what changed, how you verified it, and anything the reviewer should look at hardest.
10. If you could not do the work properly — missing tools, access, or org process you'd be guessing at — report_gap once and block with the reason. Never improvise a process to look busy.
