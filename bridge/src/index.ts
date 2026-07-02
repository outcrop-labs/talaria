/**
 * talaria-bridge — entrypoint.
 *
 * A transparent reverse-proxy of the Hermes dashboard (:9119). By default it
 * forwards EVERYTHING byte-for-byte (headers, auth, SSE streams, websocket
 * upgrades). Only requests matching the mission-dispatch allowlist (intercept.ts,
 * empty until M0) are peeled off and translated to mission-control REST.
 *
 * hermes-workspace points HERMES_DASHBOARD_URL at this process. The Hermes
 * gateway (:8642) and workspace chat/streaming never traverse the bridge.
 */
import http from "node:http";
import httpProxy from "http-proxy";

import { loadConfig, watchFleet } from "./config.js";
import { MissionControlClient } from "./missionControl.js";
import { isMissionRoute, handleMission, MISSION_ROUTES } from "./intercept.js";
import { isKanbanRoute, handleKanban } from "./kanban.js";
import { startGatewayPlane } from "./gatewayPlane.js";

const cfg = loadConfig();
const mc = new MissionControlClient(cfg);

// Verbose request logging (TALARIA_LOG_REQUESTS=1) for M1 probe capture. Conductor
// routes are ALWAYS logged (low-volume, high-signal — this is how we learn the exact
// capability-probe request the workspace sends).
const LOG_REQUESTS = process.env.TALARIA_LOG_REQUESTS === "1";

// changeOrigin:false + ws:true → preserve Host header and proxy websocket
// upgrades unchanged (byte-for-byte pass-through of the dashboard).
const proxy = httpProxy.createProxyServer({
  target: cfg.dashboardUpstream,
  changeOrigin: false,
  ws: true,
  xfwd: true,
});

// A proxy error must not take the process down — return 502 and log.
proxy.on("error", (err, _req, res) => {
  console.error(`[talaria] proxy error → ${cfg.dashboardUpstream}: ${err.message}`);
  if (res instanceof http.ServerResponse && !res.headersSent) {
    res.statusCode = 502;
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ error: "talaria: dashboard upstream unreachable" }));
  }
});

const server = http.createServer((req, res) => {
  const reqPath = req.url ?? "";
  if (LOG_REQUESTS || reqPath.startsWith("/api/conductor")) {
    console.log(`[talaria] ${req.method} ${reqPath}`);
  }

  // Lightweight liveness endpoint for docker healthchecks (never proxied).
  if (req.url === "/__talaria/health") {
    res.statusCode = 200;
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ ok: true, intercepting: MISSION_ROUTES.length }));
    return;
  }

  if (isMissionRoute(req)) {
    handleMission(req, res, mc).catch((err) => {
      console.error(`[talaria] mission handler error: ${err.message}`);
      if (!res.headersSent) {
        res.statusCode = 500;
        res.end(JSON.stringify({ error: "talaria: mission handler failed" }));
      }
    });
    return;
  }

  // Kanban plugin surface → served from mission-control (the fleet board view).
  // Only when a brain is configured + the toggle is on; otherwise pass through to
  // the real dashboard so pure-proxy mode keeps the native Hermes kanban.
  if (mc.enabled && cfg.kanbanFromMc && isKanbanRoute(req)) {
    handleKanban(req, res, mc).catch((err) => {
      console.error(`[talaria] kanban handler error: ${err.message}`);
      if (!res.headersSent) {
        res.statusCode = 500;
        res.end(JSON.stringify({ error: "talaria: kanban handler failed" }));
      }
    });
    return;
  }

  // Default path: transparent pass-through to the real dashboard.
  proxy.web(req, res);
});

// Proxy websocket upgrades straight through (dashboard live updates).
server.on("upgrade", (req, socket, head) => {
  proxy.ws(req, socket, head);
});

server.listen(cfg.port, () => {
  console.log(
    `[talaria] dashboard plane on :${cfg.port} → dashboard ${cfg.dashboardUpstream}` +
      ` | mission-control ${mc.enabled ? cfg.missionControlUrl : "(disabled)"}` +
      ` | intercepting ${MISSION_ROUTES.length} conductor route(s)` +
      ` | kanban-from-mc ${mc.enabled && cfg.kanbanFromMc ? "on" : "off"}`,
  );
});

// Second plane: the fleet multiplexer (no-op if no fleet manifest is configured).
startGatewayPlane(cfg);
// Talaria re-renders the manifest when the fleet changes — pick it up live.
watchFleet(cfg);
