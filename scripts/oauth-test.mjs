/**
 * One-shot OAuth test: localhost callback server, single exchange attempt.
 * Run: node scripts/oauth-test.mjs
 */
import http from "node:http";
import { randomBytes, createHash } from "node:crypto";

const CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const PORT = 53692;
const REDIRECT_URI = `http://localhost:${PORT}/callback`;
const TOKEN_URL = "https://platform.claude.com/v1/oauth/token";
const AUTHORIZE_URL = "https://claude.ai/oauth/authorize";
const SCOPES = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers";

const verifier = randomBytes(32).toString("base64url");
const challenge = createHash("sha256").update(verifier).digest("base64url");

const params = new URLSearchParams({
  code: "true",
  client_id: CLIENT_ID,
  response_type: "code",
  redirect_uri: REDIRECT_URI,
  scope: SCOPES,
  code_challenge: challenge,
  code_challenge_method: "S256",
  state: verifier,
});

const authUrl = `${AUTHORIZE_URL}?${params}`;

console.log("\n=== OAuth localhost callback test ===");
console.log("\nOpen this URL in your browser:\n");
console.log(authUrl);
console.log("\nWaiting for callback on", REDIRECT_URI, "...\n");

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  if (url.pathname !== "/callback") {
    res.writeHead(404); res.end("not found"); return;
  }

  const code  = url.searchParams.get("code");
  const state = url.searchParams.get("state");
  const error = url.searchParams.get("error");

  if (error) {
    res.writeHead(400, { "Content-Type": "text/plain" });
    res.end(`Auth error: ${error}`);
    console.error("Auth error:", error);
    server.close();
    return;
  }

  if (!code || !state) {
    res.writeHead(400, { "Content-Type": "text/plain" });
    res.end("Missing code or state");
    server.close();
    return;
  }

  console.log("Got callback!");
  console.log("  code: ", code);
  console.log("  state:", state);
  console.log("  state matches verifier:", state === verifier);

  res.writeHead(200, { "Content-Type": "text/html" });
  res.end("<html><body><h2>Authorized — you can close this tab.</h2></body></html>");
  server.close();

  // Exchange the code
  console.log("\nExchanging code...");
  try {
    const resp = await fetch(TOKEN_URL, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        grant_type: "authorization_code",
        client_id: CLIENT_ID,
        code,
        state,
        redirect_uri: REDIRECT_URI,
        code_verifier: verifier,
      }),
    });

    const body = await resp.text();
    console.log("\n=== Token endpoint response ===");
    console.log("HTTP status:", resp.status);
    console.log("Body:", body);

    if (resp.ok) {
      const data = JSON.parse(body);
      console.log("\n=== Token details ===");
      console.log("access_token prefix:", data.access_token?.slice(0, 20));
      console.log("refresh_token prefix:", data.refresh_token?.slice(0, 20));
      console.log("expires_in:", data.expires_in, "seconds");

      // Now test the access token against the API
      console.log("\nTesting access token against API...");
      const apiResp = await fetch("https://api.anthropic.com/v1/messages", {
        method: "POST",
        headers: {
          "Authorization": `Bearer ${data.access_token}`,
          "Content-Type": "application/json",
          "anthropic-version": "2023-06-01",
          "anthropic-beta": "oauth-2025-04-20",
        },
        body: JSON.stringify({
          model: "claude-haiku-4-5",
          max_tokens: 10,
          messages: [{ role: "user", content: "hi" }],
        }),
      });
      const apiBody = await apiResp.text();
      console.log("API status:", apiResp.status);
      console.log("API body:", apiBody.slice(0, 200));

      // Test refresh
      if (data.refresh_token) {
        console.log("\nTesting refresh token...");
        const refreshResp = await fetch(TOKEN_URL, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            grant_type: "refresh_token",
            client_id: CLIENT_ID,
            refresh_token: data.refresh_token,
          }),
        });
        const refreshBody = await refreshResp.text();
        console.log("Refresh status:", refreshResp.status);
        console.log("Refresh body:", refreshBody.slice(0, 200));
      }
    }
  } catch (err) {
    console.error("Exchange error:", err);
  }
});

server.listen(PORT, "127.0.0.1", () => {
  console.log(`Server listening on port ${PORT}`);
});
