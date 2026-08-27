# Glossary

Every term the member guides use, one line each, with the chapter that explains it. Terms in
bold throughout the guides resolve here.

| Term | In one line | Chapter |
| :--- | :--- | :--- |
| **@mention** | Type `@` plus a name in the composer; mentioning an agent brings it into the conversation | [Comms](./comms.md) |
| **Agent** | A hired teammate with a soul, memory, skills, tools, and a model for a brain | [Working with agents](./working-with-agents.md) |
| **Agent chat** | A private thread with one agent — fresh context per topic | [Comms](./comms.md) |
| **Agent guidance** | Prompt-only instructions that ride with a template into the model, never shown on the ticket | [Templates](./templates.md) |
| **Agent start** | A column where assignment counts as approval — agents only pick up work there | [Boards](./boards.md) |
| **Assistant** | The personal agent that's just yours; it writes your daily brief | [Your assistant](./personal-assistant.md) |
| **Attached decision** | The queue item your drawer instruction will act on | [Your day](./your-day.md) |
| **auto** | An unassigned slot, or an unpriced model in the ledger | [Admin: Models](./admin-models.md) |
| **Binding** | A board's or agent's standing template pick; an agent's beats the board's | [Templates](./templates.md) |
| **Board** | Tracked work as columns — tickets people and agents move to done | [Boards](./boards.md) |
| **Brain routing** | Which knowledge brain retrieves a piece of content | [Knowledge](./knowledge.md) |
| **Brief (daily)** | The assistant-written morning document — appended to, never rewritten | [Your day](./your-day.md) |
| **Chassis** | The starter bundle a new agent carries over: model tiers, tools, plugins | [Admin: Agents](./admin-agents.md) |
| **Conclude** | Summarize a relay, post and index the summary, archive it | [Comms](./comms.md) |
| **Confab guard** | The flag on a reply produced without something to ground it | [Comms](./comms.md) |
| **Delegation** | Letting your assistant answer one conversation for you — in its own name | [Your assistant](./personal-assistant.md) |
| **Depth** | Research effort: Recon (~1 min), Brief (a few), Expedition (10 min+) | [Research](./research.md) |
| **Digest (daily)** | The one-email-a-day summary of what waits — never sent when nothing does | [Your day](./your-day.md) |
| **Distill** | The automatic summarize-and-archive of an idle agent chat — the summary lands in Files | [Comms](./comms.md) |
| **Draft job** | The server-side run that reads a plan or conversation and proposes tickets | [Plan](./plan.md) |
| **Effort / Estimate** | A ticket's t-shirt size (XS–XL) and hours | [Boards](./boards.md) |
| **Explicit grant** | The default-deny rule for Manage views and app views — allowed per person | [Admin: People](./admin-people.md) |
| **Federate** | Pulling an agent that lives outside Talaria into the roster | [Admin: Agents](./admin-agents.md) |
| **Freeform** | No template — you write from scratch | [Templates](./templates.md) |
| **Gateway** | Talaria's MCP gateway, the only door agents reach tool servers through; also the OpenAI-compatible endpoint minted keys point at | [Admin: MCP](./admin-mcp-observability-apps.md) |
| **Governance** | The audit kind covering admin actions | [Admin: Observability](./admin-mcp-observability-apps.md) |
| **Handle** | The `@name` an agent or assistant is addressed by; an assistant's can't change | [Your assistant](./personal-assistant.md) |
| **Inbox** | Work → Inbox: the daily brief plus one-screen digests of everything waiting | [Your day](./your-day.md) |
| **Intake** | The column category new tickets land in | [Boards](./boards.md) |
| **Judge (QA)** | The platform's own reviewer: Pass / Revise / Escalate on a finished ticket | [Working with agents](./working-with-agents.md) |
| **Ledger** | The priced token record behind the Cost tab | [Admin: Observability](./admin-mcp-observability-apps.md) |
| **Living document** | The plan file the agent keeps current beside the conversation | [Plan](./plan.md) |
| **Marketplace** | The app catalog (Discover tab), or the MCP registry browser | [Apps](./apps.md) |
| **Member defaults** | The org-wide baseline of what plain members may do | [Admin: People](./admin-people.md) |
| **Memory** | MEMORY.md — the facts an agent keeps, versioned | [Your assistant](./personal-assistant.md) |
| **Model tier** | A named model alias; `@Name:tier` routes one reply to it | [Working with agents](./working-with-agents.md) |
| **Muse** | Talaria's designer agent — drafts agents, skills, and edits from a description | [Knowledge](./knowledge.md) |
| **OKF** | The agent-facing summary maintained on official knowledge docs | [Knowledge](./knowledge.md) |
| **Official** | Promoted into the org brain; grounds every agent's answers | [Knowledge](./knowledge.md) |
| **Pinned admin** | An admin fixed by the `AUTH_ADMIN_EMAILS` environment setting | [Admin: People](./admin-people.md) |
| **Permission** | What a person may *do* on a surface they can reach | [Admin: People](./admin-people.md) |
| **Per-user auth** | An MCP server mode where each person connects their own account | [Admin: MCP](./admin-mcp-observability-apps.md) |
| **Promote** | Mark a knowledge doc official — indexed into the org brain | [Knowledge](./knowledge.md) |
| **Provider** | A model backend — cloud API or self-hosted server | [Admin: Models](./admin-models.md) |
| **Reference** | The short code on every card, minted from the board's name (`QL-14`) | [Boards](./boards.md) |
| **Relay** | A gathering that concludes: summary posted and indexed, then archived | [Comms](./comms.md) |
| **Review (column)** | Where agent work lands for sign-off — an agent may not sign off its own | [Boards](./boards.md) |
| **Role templates** | Starter personas for agents (the Agents page) — not ticket/plan templates | [Admin: Agents](./admin-agents.md) |
| **Roll** | Zero-downtime replacement: fresh container, the old one finishes its replies | [Admin: Agents](./admin-agents.md) |
| **Roster** | The Agents page's list of the org's agents | [Admin: Agents](./admin-agents.md) |
| **Schedules (crons)** | Recurring jobs an agent runs on its own scheduler | [Your assistant](./personal-assistant.md) |
| **Secrets** | The sealed vault in Files — no preview, no export, no public tier | [Files](./files.md) |
| **Skeleton** | The markdown body of a template — the part that seeds tickets and plans | [Templates](./templates.md) |
| **Slot** | One job a model can hold (research-brief, judge, muse…) | [Admin: Models](./admin-models.md) |
| **Soul** | An agent's SOUL.md — who it is, in its own voice | [Admin: Agents](./admin-agents.md) |
| **Space** | A folder in Knowledge whose top page is its overview | [Knowledge](./knowledge.md) |
| **Status / column** | One step of a board workflow; its category (intake/active/review/done) gives it meaning | [Boards](./boards.md) |
| **Surface** | One render target of an app: work, manage, or settings | [Apps](./apps.md) |
| **Template** | The markdown skeleton a new ticket description or plan document starts from | [Templates](./templates.md) |
| **Thread** | Replies hanging off one root message; an agent @mentioned there replies in it | [Comms](./comms.md) |
| **Ticket** | A unit of board work (developers meet it as "task" in the API) | [Boards](./boards.md) |
| **Tier mention** | `@Name:tier` — routes that one reply to a model tier | [Comms](./comms.md) |
| **Tool account** | An MCP server connected as you, for your assistant to use | [Your assistant](./personal-assistant.md) |
| **Tool override** | A per-agent tool subset, narrower than the server default | [Admin: MCP](./admin-mcp-observability-apps.md) |
| **View** | Whether a person can *reach* a surface at all; also a board's saved filter preset | [Admin: People](./admin-people.md) |
| **Watcher** | Someone following a ticket without being assigned | [Boards](./boards.md) |
| **Warming** | A just-hired container coming up to health | [Working with agents](./working-with-agents.md) |
| **Workbench** | An agent's sandboxed place for real repo work — plans and PRs you approve | [Working with agents](./working-with-agents.md) |
