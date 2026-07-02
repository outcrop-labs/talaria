/**
 * Talaria bridge configuration, resolved from the environment.
 *
 * See stack/.env.example for the canonical list. Everything has a sensible
 * default so the bridge boots in pure pass-through mode with zero config.
 */
import { readFileSync, watchFile } from "node:fs";

/**
 * One agent in the fleet manifest. Talaria multiplexes a fleet of Hermes agents,
 * and supports BOTH documented deployment shapes with the same entry:
 *
 *   A) Separate Hermes installs — one gateway per agent, each on its own host
 *      (`http://agent-x:8642`). The common/canonical pattern.
 *   B) Multiple Hermes *profiles* on one host — each profile's API server runs on
 *      its own port (`http://host:8643`, `:8644`, …). Just list them as separate
 *      entries pointing at the same host, different ports; set `profile` so the
 *      forwarded `model` field targets that profile.
 *
 * For (A) or (B), Talaria routes the OpenAI `model` to the agent's gateway and
 * injects that agent's key. `profile`/`upstreamModel`/`pathPrefix` are the hooks
 * for profile-routed gateways (incl. Hermes' emerging single-endpoint multiplex,
 * see NousResearch/hermes-agent #24913, #23735).
 */
export interface FleetAgent {
  /** The model id exposed to the workspace (what the UI's switcher shows). */
  model: string;
  /** The agent's real Hermes gateway base URL (e.g. http://agent-developer:8642). */
  url: string;
  /** The agent gateway's API_SERVER_KEY (Bearer), injected on forward. */
  key: string;
  /** Optional display label / persona. */
  label?: string;
  /** Hermes profile name on a profile/multiplexed gateway. When set (and no
   *  explicit `upstreamModel`), the forwarded `model` field is rewritten to this
   *  so the upstream routes to the right profile. */
  profile?: string;
  /** Explicit override for the `model` value sent upstream. Defaults to
   *  `profile ?? model`. Use when the gateway routes by the model field. */
  upstreamModel?: string;
  /** Path prefix prepended to upstream requests (chat + sessions), e.g.
   *  `/p/<profile>` for a future single-endpoint multiplex gateway. Default: none. */
  pathPrefix?: string;
}

/** The `model` value Talaria forwards upstream for this agent (profile-aware). */
export function upstreamModelFor(a: FleetAgent): string {
  return a.upstreamModel ?? a.profile ?? a.model;
}

/** The upstream base + optional profile path prefix (no trailing slash). */
export function upstreamBase(a: FleetAgent): string {
  const prefix = (a.pathPrefix ?? "").replace(/\/$/, "");
  return `${a.url}${prefix}`;
}

export interface TalariaConfig {
  /** Port the bridge listens on. hermes-workspace's HERMES_DASHBOARD_URL points here. */
  port: number;
  /** The REAL Hermes dashboard we reverse-proxy to (e.g. http://kanban-dashboard:9119). */
  dashboardUpstream: string;
  /** mission-control base URL (the fleet brain). Empty ⇒ interception disabled. */
  missionControlUrl: string;
  /** mission-control API key (Bearer). Prefer the *_FILE docker-secret form. */
  missionControlApiKey: string;
  /** Serve the workspace kanban board from mission-control (fleet view). Default on. */
  kanbanFromMc: boolean;
  /** Gateway plane: port the fleet multiplexer listens on (workspace HERMES_API_URL → here). */
  gatewayPort: number;
  /** The fleet manifest: agents this Talaria multiplexes (empty ⇒ gateway plane disabled). */
  fleet: FleetAgent[];
  /** Default agent model for non-chat gateway calls (sessions, health, …). Defaults to fleet[0]. */
  defaultModel: string;
}

/** Load the fleet manifest from TALARIA_FLEET (JSON) or TALARIA_FLEET_FILE (path). */
function loadFleet(): FleetAgent[] {
  const filePath = process.env.TALARIA_FLEET_FILE;
  let raw = process.env.TALARIA_FLEET ?? "";
  if (!raw && filePath) {
    try {
      raw = readFileSync(filePath, "utf8");
    } catch {
      raw = "";
    }
  }
  if (!raw.trim()) return [];
  try {
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((a) => a && typeof a.model === "string" && typeof a.url === "string")
      .map((a) => ({
        model: a.model,
        url: String(a.url).replace(/\/$/, ""),
        key: a.key ?? "",
        label: a.label,
        profile: typeof a.profile === "string" ? a.profile : undefined,
        upstreamModel: typeof a.upstreamModel === "string" ? a.upstreamModel : undefined,
        pathPrefix: typeof a.pathPrefix === "string" ? a.pathPrefix : undefined,
      }));
  } catch {
    return [];
  }
}

/**
 * Hot-reload the fleet manifest: watch TALARIA_FLEET_FILE and swap cfg.fleet
 * in place on change, so Talaria can re-render the fleet (add/remove agents)
 * without a bridge restart. Mutates the array the gateway plane closes over.
 */
export function watchFleet(cfg: TalariaConfig): void {
  const filePath = process.env.TALARIA_FLEET_FILE;
  if (!filePath) return;
  // watchFile (polling) — bind-mounted files often miss inotify events.
  watchFile(filePath, { interval: 2000 }, () => {
    const next = loadFleet();
    if (!next.length) {
      console.warn(`[talaria] fleet manifest ${filePath} empty/invalid — keeping ${cfg.fleet.length} agents`);
      return;
    }
    cfg.fleet.splice(0, cfg.fleet.length, ...next);
    console.log(`[talaria] fleet manifest reloaded — ${next.length} agents (${next.map((a) => a.model).join(", ")})`);
  });
}

function readSecret(fileEnv: string, valueEnv: string): string {
  const path = process.env[fileEnv];
  if (path) {
    try {
      return readFileSync(path, "utf8").trim();
    } catch {
      /* fall through to the plain env var */
    }
  }
  return (process.env[valueEnv] ?? "").trim();
}

export function loadConfig(): TalariaConfig {
  return {
    port: Number(process.env.TALARIA_PORT ?? "9119"),
    dashboardUpstream: (process.env.TALARIA_DASHBOARD_UPSTREAM ?? "http://127.0.0.1:9119").replace(/\/$/, ""),
    missionControlUrl: (process.env.MISSION_CONTROL_URL ?? "").replace(/\/$/, ""),
    missionControlApiKey: readSecret("MISSION_CONTROL_API_KEY_FILE", "MISSION_CONTROL_API_KEY"),
    kanbanFromMc: (process.env.TALARIA_KANBAN_FROM_MC ?? "1") !== "0",
    gatewayPort: Number(process.env.TALARIA_GATEWAY_PORT ?? "8642"),
    fleet: loadFleet(),
    defaultModel: (process.env.TALARIA_DEFAULT_MODEL ?? "").trim(),
  };
}
