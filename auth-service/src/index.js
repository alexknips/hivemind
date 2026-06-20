/**
 * HiveMind Better Auth sidecar.
 *
 * Acts as the OAuth 2.0 Authorization Server (AS) for the HiveMind MCP
 * resource server.  Uses Better Auth's `mcp` plugin which provides:
 *   - RFC 7591 Dynamic Client Registration at /oauth/register
 *   - Authorization Code + PKCE flow at /oauth/authorize
 *   - Token endpoint at /oauth/token
 *   - AS discovery at /.well-known/oauth-authorization-server
 *
 * The Rust MCP resource server validates incoming Bearer tokens by calling
 * POST /api/verify-token on this service, then maps the resolved user email
 * to a HiveMind tenant in Postgres.
 *
 * Required env vars:
 *   DATABASE_URL          — same Neon Postgres as hivemind serve
 *   BETTER_AUTH_SECRET    — random secret (≥32 chars) for signing sessions
 *   BETTER_AUTH_URL       — public base URL of this service (e.g. https://auth.example.com)
 *   GITHUB_CLIENT_ID      — GitHub OAuth app client ID
 *   GITHUB_CLIENT_SECRET  — GitHub OAuth app client secret
 *   GOOGLE_CLIENT_ID      — Google OAuth app client ID
 *   GOOGLE_CLIENT_SECRET  — Google OAuth app client secret
 *
 * Optional:
 *   PORT                  — listen port (default 4000)
 *   TRUSTED_ORIGIN        — allowed CORS origin for the Rust server
 */

import { betterAuth } from "better-auth"
import { mcp } from "better-auth/plugins"
import { Pool } from "pg"
import http from "node:http"
import { toNodeHandler } from "better-auth/node"

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

const pool = new Pool({
  connectionString: process.env.DATABASE_URL,
  ssl: process.env.DATABASE_URL?.includes("neon.tech")
    ? { rejectUnauthorized: false }
    : false
})

// ---------------------------------------------------------------------------
// Better Auth
// ---------------------------------------------------------------------------

if (!process.env.BETTER_AUTH_SECRET) {
  console.error("BETTER_AUTH_SECRET env var is required")
  process.exit(1)
}

const auth = betterAuth({
  baseURL: process.env.BETTER_AUTH_URL || `http://localhost:${process.env.PORT || 4000}`,
  secret: process.env.BETTER_AUTH_SECRET,
  database: pool,
  socialProviders: {
    github: {
      clientId: process.env.GITHUB_CLIENT_ID || "",
      clientSecret: process.env.GITHUB_CLIENT_SECRET || ""
    },
    google: {
      clientId: process.env.GOOGLE_CLIENT_ID || "",
      clientSecret: process.env.GOOGLE_CLIENT_SECRET || ""
    }
  },
  plugins: [
    mcp({ loginPage: "/auth/login" })
  ],
  trustedOrigins: process.env.TRUSTED_ORIGIN
    ? [process.env.TRUSTED_ORIGIN]
    : []
})

const betterAuthHandler = toNodeHandler(auth)

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function readBody (req) {
  return new Promise((resolve, reject) => {
    const chunks = []
    req.on("data", chunk => chunks.push(chunk))
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")))
    req.on("error", reject)
  })
}

function sendJson (res, data, status = 200) {
  const body = JSON.stringify(data)
  res.writeHead(status, {
    "Content-Type": "application/json",
    "Content-Length": Buffer.byteLength(body)
  })
  res.end(body)
}

// ---------------------------------------------------------------------------
// Custom endpoints
// ---------------------------------------------------------------------------

/**
 * POST /api/verify-token
 * Body: { "token": "<bearer token issued by Better Auth>" }
 * Response: { "valid": bool, "email": string, "user_id": string }
 *
 * Called by the Rust resource server to validate incoming MCP Bearer tokens
 * before mapping them to a HiveMind tenant.
 */
async function handleVerifyToken (req, res) {
  let parsed
  try {
    parsed = JSON.parse(await readBody(req))
  } catch {
    return sendJson(res, { valid: false, error: "invalid JSON" }, 400)
  }

  const { token } = parsed
  if (!token || typeof token !== "string") { // ubs:ignore - type check, not a value comparison
    return sendJson(res, { valid: false })
  }

  // Attempt 1: Better Auth session API (covers session Bearer tokens).
  try {
    const session = await auth.api.getSession({
      headers: new Headers({ Authorization: `Bearer ${token}` })
    })
    if (session?.user?.email) {
      return sendJson(res, {
        valid: true,
        email: session.user.email,
        user_id: session.user.id
      })
    }
  } catch (_) { /* fall through */ }

  // Attempt 2: query the Better Auth MCP access-token table directly.
  // Table and column names reflect Better Auth's internal schema for the mcp plugin.
  for (const query of [
    // mcp plugin schema
    `SELECT mat.user_id, u.email
       FROM mcp_access_token mat
       JOIN "user" u ON mat.user_id = u.id
      WHERE mat.token = $1 AND mat.expires_at > NOW()`,
    // oAuth provider / generic oauth_access_token table
    `SELECT oat.user_id, u.email
       FROM oauth_access_token oat
       JOIN "user" u ON oat.user_id = u.id
      WHERE oat.access_token = $1 AND oat.expires_at > NOW()`
  ]) {
    try {
      const result = await pool.query(query, [token])
      if (result.rows.length > 0) {
        return sendJson(res, {
          valid: true,
          email: result.rows[0].email,
          user_id: result.rows[0].user_id
        })
      }
    } catch (_) { /* table may not exist yet */ }
  }

  sendJson(res, { valid: false })
}

function serveLoginPage (res) {
  const html = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>HiveMind Login</title>
  <style>
    body { font-family: sans-serif; max-width: 400px; margin: 4rem auto; padding: 0 1rem; }
    h1 { font-size: 1.5rem; }
    a.btn {
      display: block; padding: .75rem 1rem; margin: .5rem 0;
      background: #24292e; color: #fff; text-decoration: none;
      border-radius: 6px; text-align: center;
    }
    a.btn.google { background: #4285f4; }
  </style>
</head>
<body>
  <h1>HiveMind</h1>
  <p>Connect your coding agent to HiveMind decision memory.</p>
  <a class="btn" href="/api/auth/sign-in/github">Login with GitHub</a>
  <a class="btn google" href="/api/auth/sign-in/google">Login with Google</a>
</body>
</html>`
  res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" })
  res.end(html)
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

const server = http.createServer(async (req, res) => {
  const url = req.url || "/"
  const method = req.method || "GET"

  if (url === "/api/verify-token" && method === "POST") { // ubs:ignore - URL path comparison, not a bearer token
    return handleVerifyToken(req, res)
  }

  if ((url === "/" || url === "/auth/login") && method === "GET") {
    return serveLoginPage(res)
  }

  return betterAuthHandler(req, res)
})

const port = Number(process.env.PORT || 4000)
server.listen(port, () => {
  const base = process.env.BETTER_AUTH_URL || `http://localhost:${port}`
  console.log(`HiveMind Better-Auth service listening on port ${port}`)
  console.log(`Base URL: ${base}`)
  console.log(`AS metadata: ${base}/.well-known/oauth-authorization-server`)
  console.log(`Token verify: POST ${base}/api/verify-token`) // ubs:ignore - URL path only, no token values
})
