/**
 * Gateway unit tests — mock fetch to verify HTTP routing without a live server.
 * Run with: npm run test:ts
 */

import assert from "node:assert/strict";
import { test, before, beforeEach } from "node:test";
import {
  getDecision,
  getRelevantDecisions,
  getSupersessionChain,
  searchDecisions,
} from "../src/client.ts";

// ---------------------------------------------------------------------------
// Fetch mock
// ---------------------------------------------------------------------------

interface CapturedCall {
  url: string;
  authHeader: string;
}

let lastCall: CapturedCall | null = null;

before(() => {
  globalThis.fetch = async (input: string | URL | Request, init?: RequestInit) => {
    const url = input instanceof Request ? input.url : input.toString();
    const headers = (init?.headers ?? {}) as Record<string, string>;
    lastCall = { url, authHeader: headers["Authorization"] ?? "" };
    return {
      ok: true,
      status: 200,
      json: async () => ({ data: [], meta: {} }),
    } as Response;
  };
});

beforeEach(() => {
  lastCall = null;
});

const BASE = "http://hm.test";
const KEY = "hm_sk_live_test";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test("getDecision routes to /v1/decisions/:id", async () => {
  await getDecision(BASE, KEY, "abc-123");
  assert.ok(lastCall?.url.includes("/v1/decisions/abc-123"), `url: ${lastCall?.url}`);
});

test("getDecision forwards bearer token", async () => {
  await getDecision(BASE, KEY, "abc-123");
  assert.equal(lastCall?.authHeader, `Bearer ${KEY}`);
});

test("getRelevantDecisions routes to /v1/decisions/relevant with topic", async () => {
  await getRelevantDecisions(BASE, KEY, "auth");
  assert.ok(lastCall?.url.includes("/v1/decisions/relevant"), `url: ${lastCall?.url}`);
  assert.ok(lastCall?.url.includes("topic=auth"), `url: ${lastCall?.url}`);
});

test("getRelevantDecisions forwards optional status filter", async () => {
  await getRelevantDecisions(BASE, KEY, "auth", "accepted");
  assert.ok(lastCall?.url.includes("status=accepted"), `url: ${lastCall?.url}`);
});

test("getSupersessionChain routes to /v1/decisions/:id/supersession-chain", async () => {
  await getSupersessionChain(BASE, KEY, "dec-456");
  assert.ok(
    lastCall?.url.includes("/v1/decisions/dec-456/supersession-chain"),
    `url: ${lastCall?.url}`
  );
});

test("searchDecisions routes to /v1/decisions/search", async () => {
  await searchDecisions(BASE, KEY, { q: "postgres migration", limit: 10 });
  assert.ok(lastCall?.url.includes("/v1/decisions/search"), `url: ${lastCall?.url}`);
  assert.ok(lastCall?.url.includes("q=postgres"), `url: ${lastCall?.url}`);
  assert.ok(lastCall?.url.includes("limit=10"), `url: ${lastCall?.url}`);
});

test("searchDecisions omits empty params", async () => {
  await searchDecisions(BASE, KEY, { q: "test" });
  const url = lastCall?.url ?? "";
  assert.ok(!url.includes("topic="), `should not include empty topic: ${url}`);
  assert.ok(!url.includes("status="), `should not include empty status: ${url}`);
});

test("bearer token is forwarded on every read call", async () => {
  for (const call of [
    () => getDecision(BASE, KEY, "x"),
    () => getRelevantDecisions(BASE, KEY, "x"),
    () => getSupersessionChain(BASE, KEY, "x"),
    () => searchDecisions(BASE, KEY, {}),
  ]) {
    await call();
    assert.equal(
      lastCall?.authHeader,
      `Bearer ${KEY}`,
      `bearer not forwarded in ${lastCall?.url}`
    );
  }
});

test("getDecision encodes special characters in id", async () => {
  await getDecision(BASE, KEY, "dec/with spaces");
  assert.ok(
    lastCall?.url.includes("dec%2Fwith%20spaces") ||
    lastCall?.url.includes("dec/with%20spaces"),
    `url: ${lastCall?.url}`
  );
});

test("getRelevantDecisions requires non-empty topic", () => {
  assert.throws(
    () => getRelevantDecisions(BASE, KEY, "  "),
    { message: /topic is required/ }
  );
});

test("getDecision requires non-empty id", () => {
  assert.throws(
    () => getDecision(BASE, KEY, ""),
    { message: /decision_id is required/ }
  );
});
