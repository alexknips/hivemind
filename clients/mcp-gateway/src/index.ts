#!/usr/bin/env node
/**
 * HiveMind MCP gateway — thin stdio transport over the HiveMind HTTP API.
 *
 * Maps MCP tool calls to /v1 read endpoints. No business logic here.
 * Auth: HIVEMIND_API_KEY bearer token forwarded to the service on every call.
 * Tenant scope is enforced server-side (RLS); gateway stays tenant-agnostic.
 *
 * Required env vars:
 *   HIVEMIND_URL      Base URL of the HiveMind HTTP service (no trailing slash)
 *   HIVEMIND_API_KEY  Bearer token (hm_sk_live_...)
 */

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  Tool,
} from "@modelcontextprotocol/sdk/types.js";
import {
  getDecision,
  getRelevantDecisions,
  getSupersessionChain,
  searchDecisions,
} from "./client.js";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

function requireEnv(name: string): string {
  const val = process.env[name];
  if (!val) {
    throw new Error(`Missing required environment variable: ${name}`);
  }
  return val;
}

const HIVEMIND_URL = requireEnv("HIVEMIND_URL").replace(/\/$/, "");
const HIVEMIND_API_KEY = requireEnv("HIVEMIND_API_KEY");

// ---------------------------------------------------------------------------
// Tool definitions (read path only; input schemas match the Rust MCP server)
// ---------------------------------------------------------------------------

const TOOLS: Tool[] = [
  {
    name: "get_decision",
    description: "Fetch a single decision by id. Returns null when absent.",
    inputSchema: {
      type: "object" as const,
      required: ["decision_id"],
      properties: {
        decision_id: { type: "string" },
      },
    },
  },
  {
    name: "get_relevant_decisions",
    description:
      "List decisions whose topic_keys contain the given topic. Optional status filter.",
    inputSchema: {
      type: "object" as const,
      required: ["topic"],
      properties: {
        topic: { type: "string" },
        status: {
          type: "string",
          enum: [
            "proposed",
            "accepted",
            "rejected",
            "contested",
            "superseded",
          ],
        },
      },
    },
  },
  {
    name: "get_supersession_chain",
    description:
      "Return the linear supersession chain a decision sits in, oldest first.",
    inputSchema: {
      type: "object" as const,
      required: ["decision_id"],
      properties: {
        decision_id: { type: "string" },
      },
    },
  },
  {
    name: "search_decisions",
    description:
      "Full-text search over decisions. All parameters are optional.",
    inputSchema: {
      type: "object" as const,
      properties: {
        q: { type: "string", description: "Full-text query." },
        topic: {
          type: "string",
          description: "Comma-separated topic keys to filter by.",
        },
        status: {
          type: "string",
          description: "Comma-separated statuses to filter by.",
        },
        since: {
          type: "string",
          description: "RFC3339 lower bound for decision proposal time.",
        },
        until: {
          type: "string",
          description: "RFC3339 upper bound for decision proposal time.",
        },
        limit: {
          type: "number",
          description: "Maximum results to return (default 25, max 1000).",
        },
      },
    },
  },
];

// ---------------------------------------------------------------------------
// Server bootstrap
// ---------------------------------------------------------------------------

async function main() {
  const server = new Server(
    { name: "hivemind", version: "0.1.0" },
    { capabilities: { tools: {} } }
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: TOOLS,
  }));

  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    const { name, arguments: args = {} } = req.params;
    try {
      let result;
      switch (name) {
        case "get_decision":
          result = await getDecision(
            HIVEMIND_URL,
            HIVEMIND_API_KEY,
            args["decision_id"] as string
          );
          break;
        case "get_relevant_decisions":
          result = await getRelevantDecisions(
            HIVEMIND_URL,
            HIVEMIND_API_KEY,
            args["topic"] as string,
            args["status"] as string | undefined
          );
          break;
        case "get_supersession_chain":
          result = await getSupersessionChain(
            HIVEMIND_URL,
            HIVEMIND_API_KEY,
            args["decision_id"] as string
          );
          break;
        case "search_decisions":
          result = await searchDecisions(HIVEMIND_URL, HIVEMIND_API_KEY, {
            q: args["q"] as string | undefined,
            topic: args["topic"] as string | undefined,
            status: args["status"] as string | undefined,
            since: args["since"] as string | undefined,
            until: args["until"] as string | undefined,
            limit: args["limit"] as number | undefined,
          });
          break;
        default:
          throw new Error(`Unknown tool: ${name}`);
      }
      return {
        content: [
          { type: "text" as const, text: JSON.stringify(result, null, 2) },
        ],
      };
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      return {
        content: [{ type: "text" as const, text: `Error: ${message}` }],
        isError: true,
      };
    }
  });

  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((err) => {
  console.error("Fatal:", err);
  process.exit(1);
});
