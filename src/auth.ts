import { readFile, writeFile, mkdir } from "fs/promises";
import { join } from "path";
import { randomBytes, createHash } from "crypto";

// OAuth configuration matching Claude Code's flow
const OAUTH_CONFIG = {
  clientId: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
  authorizeUrl: "https://platform.claude.com/oauth/authorize",
  tokenUrl: "https://platform.claude.com/v1/oauth/token",
  callbackUrl: "https://platform.claude.com/oauth/code/callback",
  scopes: ["org:create_api_key", "user:inference", "user:profile"],
};

const TOKEN_FILE = join(
  process.env.HOME ?? "~",
  ".config",
  "omega",
  "oauth-token.json"
);

interface TokenData {
  access_token: string;
  refresh_token?: string;
  expires_at?: number; // unix timestamp
}

// Generate PKCE challenge
function generatePKCE(): { verifier: string; challenge: string } {
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256")
    .update(verifier)
    .digest("base64url");
  return { verifier, challenge };
}

// Save token to disk
async function saveToken(token: TokenData): Promise<void> {
  await mkdir(join(process.env.HOME ?? "~", ".config", "omega"), {
    recursive: true,
  });
  await writeFile(TOKEN_FILE, JSON.stringify(token, null, 2), "utf-8");
}

// Load token from disk
async function loadToken(): Promise<TokenData | null> {
  try {
    const data = await readFile(TOKEN_FILE, "utf-8");
    return JSON.parse(data);
  } catch {
    return null;
  }
}

// Refresh the access token using refresh_token
async function refreshToken(token: TokenData): Promise<TokenData | null> {
  if (!token.refresh_token) return null;

  try {
    const resp = await fetch(OAUTH_CONFIG.tokenUrl, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "refresh_token",
        client_id: OAUTH_CONFIG.clientId,
        refresh_token: token.refresh_token,
      }),
    });

    if (!resp.ok) return null;

    const data = await resp.json() as any;
    const newToken: TokenData = {
      access_token: data.access_token,
      refresh_token: data.refresh_token ?? token.refresh_token,
      expires_at: data.expires_in
        ? Math.floor(Date.now() / 1000) + data.expires_in
        : undefined,
    };
    await saveToken(newToken);
    return newToken;
  } catch {
    return null;
  }
}

// Start the OAuth authorization code flow with PKCE
// Returns a URL the user must open and a function to poll/exchange the code
export async function startOAuthFlow(): Promise<{
  url: string;
  exchangeCode: (code: string) => Promise<TokenData>;
}> {
  const { verifier, challenge } = generatePKCE();
  const state = randomBytes(16).toString("hex");

  const params = new URLSearchParams({
    client_id: OAUTH_CONFIG.clientId,
    response_type: "code",
    redirect_uri: OAUTH_CONFIG.callbackUrl,
    scope: OAUTH_CONFIG.scopes.join(" "),
    state,
    code_challenge: challenge,
    code_challenge_method: "S256",
  });

  const url = `${OAUTH_CONFIG.authorizeUrl}?${params}`;

  const exchangeCode = async (code: string): Promise<TokenData> => {
    const body = {
      grant_type: "authorization_code",
      code,
      redirect_uri: OAUTH_CONFIG.callbackUrl,
      client_id: OAUTH_CONFIG.clientId,
      code_verifier: verifier,
      state,
    };

    const resp = await fetch(OAUTH_CONFIG.tokenUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });

    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(`Token exchange failed (${resp.status}): ${text}`);
    }

    const data = await resp.json() as any;
    const token: TokenData = {
      access_token: data.access_token,
      refresh_token: data.refresh_token,
      expires_at: data.expires_in
        ? Math.floor(Date.now() / 1000) + data.expires_in
        : undefined,
    };
    await saveToken(token);
    return token;
  };

  return { url, exchangeCode };
}

const API_KEY_FILE = join(
  process.env.HOME ?? "~",
  ".config",
  "omega",
  "api-key"
);

// Exchange OAuth token for an API key (Claude Max billing)
// This is the critical step: Claude Code does this same call to
// /api/oauth/claude_cli/create_api_key to get a key that bills
// through the Max subscription instead of per-token.
async function createApiKeyFromOAuth(accessToken: string): Promise<string | null> {
  try {
    const resp = await fetch(
      "https://api.anthropic.com/api/oauth/claude_cli/create_api_key",
      {
        method: "POST",
        headers: {
          Authorization: `Bearer ${accessToken}`,
          "Content-Type": "application/json",
        },
      }
    );

    if (!resp.ok) return null;

    const data = (await resp.json()) as any;
    return data.raw_key ?? null;
  } catch {
    return null;
  }
}

async function saveApiKey(key: string): Promise<void> {
  await mkdir(join(process.env.HOME ?? "~", ".config", "omega"), {
    recursive: true,
  });
  await writeFile(API_KEY_FILE, key, "utf-8");
}

async function loadApiKey(): Promise<string | null> {
  try {
    return (await readFile(API_KEY_FILE, "utf-8")).trim();
  } catch {
    return null;
  }
}

// Get a valid OAuth access token, refreshing if needed
async function getValidAccessToken(): Promise<string | null> {
  const token = await loadToken();
  if (!token) return null;

  // Check expiry (with 5 min buffer)
  if (token.expires_at && token.expires_at < Date.now() / 1000 + 300) {
    const refreshed = await refreshToken(token);
    if (refreshed) return refreshed.access_token;
    return null;
  }

  return token.access_token;
}

// Get a working API key derived from OAuth (Claude Max billing).
// Flow: OAuth token → /create_api_key → API key
// The API key is cached to disk so we don't create a new one every startup.
export async function getApiKey(): Promise<string | null> {
  // Try cached API key first
  const cached = await loadApiKey();
  if (cached) return cached;

  // No cached key — need OAuth token to create one
  const accessToken = await getValidAccessToken();
  if (!accessToken) return null;

  const apiKey = await createApiKeyFromOAuth(accessToken);
  if (apiKey) {
    await saveApiKey(apiKey);
    return apiKey;
  }

  return null;
}

// Legacy: get raw OAuth token (for fallback / debugging)
export async function getAuthToken(): Promise<string | null> {
  return getValidAccessToken();
}

// Verify that an API key actually works and check billing type.
// Makes a minimal API call (count tokens for a tiny message) and inspects
// the response. Returns { valid, billing } where billing is "max", "api-key",
// or "unknown".
export async function verifyApiKey(apiKey: string): Promise<{
  valid: boolean;
  billing: "max" | "api-key" | "unknown";
  error?: string;
}> {
  try {
    // Use the count_tokens endpoint — cheapest possible API call (no tokens billed)
    const resp = await fetch("https://api.anthropic.com/v1/messages/count_tokens", {
      method: "POST",
      headers: {
        "x-api-key": apiKey,
        "anthropic-version": "2023-06-01",
        "content-type": "application/json",
      },
      body: JSON.stringify({
        model: "claude-sonnet-4-20250514",
        messages: [{ role: "user", content: "hi" }],
      }),
    });

    if (!resp.ok) {
      const text = await resp.text();
      if (text.includes("usage limits")) {
        return { valid: false, billing: "api-key", error: "API usage limit reached" };
      }
      return { valid: false, billing: "unknown", error: `${resp.status}: ${text.slice(0, 200)}` };
    }

    // Check rate limit headers — Max accounts have much higher limits
    const rateLimit = resp.headers.get("x-ratelimit-limit-requests");
    const rateLimitTokens = resp.headers.get("x-ratelimit-limit-input-tokens");

    // Tier 1 pay-per-token: 50 req/min, 30k input tokens/min
    // Max accounts: much higher (typically 1000+ req/min)
    if (rateLimit) {
      const limit = parseInt(rateLimit, 10);
      if (limit > 200) {
        return { valid: true, billing: "max" };
      } else {
        return { valid: true, billing: "api-key" };
      }
    }

    return { valid: true, billing: "unknown" };
  } catch (err: any) {
    return { valid: false, billing: "unknown", error: err.message };
  }
}

export { TOKEN_FILE, API_KEY_FILE, type TokenData };
