# How agents reach Talaria, and why that needed a firewall rule

Agents run in containers; the app runs on the host. Every call an agent makes
back to Talaria crosses that boundary, and on a default-deny host it needs one
firewall rule — which the preflight detects, prints, and verifies from where
the agent stands.

(First diagnosed 2026-08-17, when every managed agent spent a day healthy and
toolless: containers green, `/health` answering 200, roster clean, and not one
of them able to call a tool — the only evidence one unread startup line, which
is the string to grep for when it happens again:)

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

## Why the alerting needed the preflight

The alerts engine (`api/src/alerts.rs`) probes the toolkit at `127.0.0.1:5280`,
and it answers perfectly — because **the app can always reach itself**.

> Reachability is not a property of a service. It is a property of a path.

Testing the agent's path requires standing where the agent stands.
`api/src/fleet/preflight.rs` does that: it runs a throwaway container on the
fleet network and asks *it*. That verdict is what the "Agents cannot reach
Talaria" alert reports, and it runs when an agent is brought up.

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
to `docker` (`api/src/fleet/docker.rs`) — it renders compose files, starts and
stops containers, rolls slots. Containerising the app therefore means giving
that container the docker socket, and the workbench mount policy refuses
exactly that mount for exactly the right reason:

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

## What the preflight checks

It stands where the agent stands, and it asks more than "can a container open a
socket to the app": it checks the app port the rendered configs point at, the
actual `/api/mcp/gw` target an agent's MCP client dials, and whether the fleet
network can resolve an external name, probed with the same resolvers the
chassis gives agents. The all-clear reads "agents can reach the app, the
toolkit, and the internet", and each failure names its own fix.

## Agents carry explicit DNS

Docker pins a network's DNS upstream when the network is created, and a host
resolver mix that refuses docker subnets leaves every container green and
offline — the built-in browser shipped dead exactly this way once: its engine
comes from npm (today it is installed before an agent with the browser and the
terminal on starts, in a one-shot run of the agent's own service, so with these
same resolvers), and with no external DNS it never installed, silently. The
chassis pins external resolvers per service (`AGENT_DNS_1` /
`AGENT_DNS_2` in `fleet/.env`, defaults 1.1.1.1 / 1.0.0.1), and the
preflight's DNS probe turns a dead resolver into an alert.

## What an agent's browser can reach

An agent's container reaches what the host's docker bridge reaches: the Talaria app and
toolkit at `host.docker.internal`, anything else the host serves on its bridge addresses,
the other containers on the fleet network, and the LAN. Hermes refuses private and internal
addresses to its web tools (`web_extract` resolves every host and refuses a private one).
It does not refuse them to its own browser when that browser runs locally: Hermes v0.22.4
skips the check whenever the terminal backend is local, on the grounds that the terminal
reaches the same networks anyway. It judges that by the terminal backend, not by whether
the agent has a terminal at all.

So Talaria gives the browser only to an agent whose `terminal` toolset is on, and for that
agent the premise holds: its shell already reaches everything its browser can. An agent
with `browser` ticked and the terminal closed, which is a new agent's default, gets no
browser engine and no browser tool, and the toolset grid says why. Its rendered config is
the one it had before Talaria installed engines at all, so an upgrade changes nothing for
it. Opening its terminal gives it Hermes' `browser_exec` and the shell together; closing it
again takes both away on the next roll.

A def's own `browser:` block is its owner's call, and the render passes it through as
written. Hermes reads any `browser.backend` other than `browser-use` as its built-in tools.
On this machine's Chromium they need the same engine, and Talaria installs none for a
terminal-less agent, but an engine that an earlier start with the terminal open left on the
agent's volume stays there and would run them; Camofox (`browser.cloud_provider: camofox`, or
a `CAMOFOX_URL` secret with no provider written) needs no engine at all. Hermes calls both of
those local and skips the check for them. So with the terminal closed, the render closes the
`browser` toolset for an agent whose config picks one of them (`fleet_browser_local_close`),
and the grid says so on that agent. A CDP endpoint (`browser.cdp_url`) or a hosted provider
(`browser.cloud_provider`) keeps Hermes' check and is left alone. The render reads the
terminal where Hermes does: the toolset grid's list, and a `platform_toolsets.api_server` list
a def authored, which replaces Hermes' default bundle (the grid does not show that one). What
it cannot see (a `BROWSER_CDP_URL` secret, a hosted provider's credentials in the env) it
reads as absent, which errs toward closing. The boundaries for what any agent may reach are:

- the `browser` and `terminal` toolsets off, for an agent that must not reach your network;
- host services bound to loopback only (the chassis note on `host.docker.internal`); and
- the host's own firewall, for what the fleet subnet may reach on the LAN.
