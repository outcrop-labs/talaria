# How agents reach Talaria, and why that needed a firewall rule

_Written 2026-08-17, after every managed agent spent a day healthy and toolless._

## The symptom

Agents were green. Containers up, `/health` answering 200, roster clean. And not
one of them could call a tool, or produce anything longer than a short reply.

The only evidence was one line, logged once at startup, that nothing read:

```
WARNING tools.mcp_tool: Failed to connect to MCP server 'talaria': CancelledError
```

## The cause

The app runs on the **host**. Agents run in **containers**. So every call an
agent makes back to Talaria — the MCP toolkit for tools, the LLM gateway for
thinking — crosses the container→host boundary.

That traffic goes through the host's **INPUT** chain. Docker does not manage
INPUT; it manages `FORWARD` (container→outside) and `DOCKER`/`DOCKER-USER` (port
publishing). So on a host with a default-deny firewall:

```
-P INPUT DROP
```

…every Docker rule is correct, the app is listening on all interfaces, the
container has working DNS and full internet access — and the packets are
dropped. Because it is DROP rather than REJECT, the agent *hangs* instead of
erroring, so the failure surfaces as timeouts and empty tool results rather than
as a connection error anyone would grep for.

`ssh` kept working throughout, which made it look like the network was fine.
It worked because the firewall had a rule for port 22 and none for 5273.

## Why the existing alerting missed it

`alerts.ts` probes the toolkit at `127.0.0.1:5280`. It answered perfectly, all
day, because **the app can always reach itself**.

> Reachability is not a property of a service. It is a property of a path.

Testing the agent's path requires standing where the agent stands.
`server/fleet-preflight.ts` does that: it runs a throwaway container on the fleet
network and asks *it*. That verdict is what the "Agents cannot reach Talaria"
alert reports, and it runs when an agent is brought up.

## The immediate fix

Admit the docker bridges to the host's app and toolkit ports. The preflight
detects which firewall the host runs and prints the matching command — `ufw`,
`firewall-cmd`, `nft`, or a plain description of the property to satisfy if it
recognises none of them.

```
sudo ufw allow from 172.16.0.0/12 to any port 5273 proto tcp
sudo ufw allow from 172.16.0.0/12 to any port 5280 proto tcp
```

By **subnet**, not by interface: bridge names like `br-4ce33ca810a3` change
whenever a docker network is recreated, and `172.16.0.0/12` covers every docker
bridge. This is narrower than exposing the ports to the LAN — only containers
can use it.

**An agent connects its MCP servers at startup and does not retry**, so an agent
that started while blocked needs a restart after the rule lands. Otherwise it
stays toolless with a clean log.

## The right shape, and the reason we are not in it

The firewall rule is a workaround for an architectural fact: **the app is not on
the fleet network.** If it were, agent→app would be container→container — which
Docker manages, needs no host rule, and works identically on every distro. The
only firewall openings left would be the ones you actually want: the web UI, and
any agent endpoint you deliberately expose.

The codebase already anticipates this. `TALARIA_GATEWAY_SELF_URL` exists for it,
and the chassis says so directly:

> In a fully containerized stack, drop this and set `TALARIA_GATEWAY_SELF_URL`
> to the app's service DNS instead.

**The obstacle is the docker socket.** Talaria manages the fleet by shelling out
to `docker` (`fleet-docker.ts`) — it renders compose files, starts and stops
containers, rolls slots. Containerising the app therefore means giving that
container the docker socket, and `workbench.ts` refuses exactly that mount for
exactly the right reason:

> The docker socket is host root by another name.

So the choice is real, not an oversight:

| | Agent→app path | Host firewall | Control plane |
|---|---|---|---|
| **App on host** (today) | container→host, INPUT | needs a rule | direct `docker` CLI |
| **App containerised** | container→container | none needed | needs the docker socket, i.e. host root |

Neither is free. The options worth weighing, if this is taken further:

1. **Split the control plane.** A small privileged sidecar holds the socket and
   exposes only the fleet verbs Talaria needs; the app runs unprivileged on the
   fleet network. Best end state, most work.
2. **Rootless Docker or a socket proxy.** Narrows what the socket grants without
   restructuring the app.
3. **Stay on the host and make the rule a first-class install step** — detected,
   explained, and verified by the preflight rather than discovered.

Today we are at (3), and the preflight is what makes it survivable. Anyone
picking this up should read (1) as the target rather than assuming the firewall
rule is the intended design.

## What changed since (2026-08-25)

Two quiet assumptions in the story above have since been fixed in code.

**The preflight probes the whole path now.** It still stands where the agent
stands, but it no longer asks only "can a container open a socket to the app":
it checks the app port the rendered configs point at, the actual `/api/mcp/gw`
target an agent's MCP client dials (it previously probed the toolkit's
standalone listener on 5280, a path no agent takes), and whether the fleet
network can resolve an external name, probed with the same resolvers the
chassis gives agents. The all-clear reads "agents can reach the app, the
toolkit, and the internet", and each failure names its own fix.

**Agents carry explicit DNS.** Docker pins a network's DNS upstream when the
network is created, and a host resolver mix that refuses docker subnets leaves
every container green and offline. That is how the built-in browser shipped
dead: the browser toolset fetches its engine from npm on first use, so with no
external DNS it never installed, silently. The chassis now pins external
resolvers per service (`AGENT_DNS_1` / `AGENT_DNS_2` in `fleet/.env`, defaults
1.1.1.1 / 1.0.0.1), and the preflight's DNS probe turns a dead resolver into
an alert.
