/**
 * Thin HTTP client over the HiveMind /v1 read endpoints.
 * Separated from the MCP server bootstrap so tests can import it independently.
 */

export type ApiResult = Record<string, unknown> | unknown[];

async function apiGet(
  baseUrl: string,
  apiKey: string,
  path: string,
  params?: Record<string, string>
): Promise<ApiResult> {
  const url = new URL(baseUrl + path);
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== "") url.searchParams.set(k, v);
    }
  }

  const res = await fetch(url.toString(), {
    headers: { Authorization: `Bearer ${apiKey}` },
  });

  const body = (await res.json()) as Record<string, unknown>;

  if (!res.ok) {
    const err = body?.error as Record<string, string> | undefined;
    throw new Error(err?.message ?? `HTTP ${res.status}: ${path}`);
  }

  return body;
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

export function getDecision(
  baseUrl: string,
  apiKey: string,
  decisionId: string
): Promise<ApiResult> {
  if (!decisionId.trim()) throw new Error("decision_id is required");
  return apiGet(baseUrl, apiKey, `/v1/decisions/${encodeURIComponent(decisionId)}`);
}

export function getRelevantDecisions(
  baseUrl: string,
  apiKey: string,
  topic: string,
  status?: string
): Promise<ApiResult> {
  if (!topic.trim()) throw new Error("topic is required");
  const params: Record<string, string> = { topic };
  if (status) params["status"] = status;
  return apiGet(baseUrl, apiKey, "/v1/decisions/relevant", params);
}

export function getSupersessionChain(
  baseUrl: string,
  apiKey: string,
  decisionId: string
): Promise<ApiResult> {
  if (!decisionId.trim()) throw new Error("decision_id is required");
  return apiGet(
    baseUrl,
    apiKey,
    `/v1/decisions/${encodeURIComponent(decisionId)}/supersession-chain`
  );
}

export function searchDecisions(
  baseUrl: string,
  apiKey: string,
  args: {
    q?: string;
    topic?: string;
    status?: string;
    since?: string;
    until?: string;
    limit?: number;
  }
): Promise<ApiResult> {
  const params: Record<string, string> = {};
  if (args.q) params["q"] = args.q;
  if (args.topic) params["topic"] = args.topic;
  if (args.status) params["status"] = args.status;
  if (args.since) params["since"] = args.since;
  if (args.until) params["until"] = args.until;
  if (args.limit !== undefined) params["limit"] = String(args.limit);
  return apiGet(baseUrl, apiKey, "/v1/decisions/search", params);
}
