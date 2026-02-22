import { readFile, writeFile, mkdir } from "fs/promises";
import { join } from "path";
import { randomBytes, createHash } from "crypto";

// OAuth configuration for Claude Max (from pi-ai's anthropic.js)
// CRITICAL: Must use claude.ai + console.anthropic.com endpoints.
// The access_token IS the API key — no create_api_key step needed.
const OAUTH_CONFIG = {
  clientId: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
  authorizeUrl: "https://claude.ai/oauth/authorize",
  tokenUrl: "https://console.anthropic.com/v1/oauth/token",
  redirectUri: "https://console.anthropic.com/oauth/code/callback",
  scopes: "org:create_api_key user:profile user:inference",
};

const CONFIG_DIR = join(process.env.HOME ?? "~", ".config", "omega");
const TOKEN_FILE = join(CONFIG_DIR, "oauth-token.json");

interface TokenData {
  access_token: string;
  refresh_token?: string;
  expires_at?: number; // unix ms timestamp
}

// Generate PKCE challenge
function generatePKCE(): { verifier: string; challenge: string } {
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256")
    .update(verifier)
    .digest("base64url");
  return { verifier, challenge };
}

async function ensureConfigDir(): Promise<void> {
  await mkdir(CONFIG_DIR, { recursive: true });
}

async function saveToken(token: TokenData): Promise<void> {
  await ensureConfigDir();
  await writeFile(TOKEN_FILE, JSON.stringify(token, null, 2), "utf-8");
}

async function loadToken(): Promise<TokenData | null> {
  try {
    const data = await readFile(TOKEN_FILE, "utf-8");
    return JSON.parse(data);
  } catch {
    return null;
  }
}

// Refresh the access token
async function refreshToken(token: TokenData): Promise<TokenData | null> {
  if (!token.refresh_token) return null;

  try {
    const resp = await fetch(OAUTH_CONFIG.tokenUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        grant_type: "refresh_token",
        client_id: OAUTH_CONFIG.clientId,
        refresh_token: token.refresh_token,
      }),
    });

    if (!resp.ok) return null;

    const data = (await resp.json()) as any;
    const newToken: TokenData = {
      access_token: data.access_token,
      refresh_token: data.refresh_token ?? token.refresh_token,
      expires_at: Date.now() + data.expires_in * 1000 - 5 * 60 * 1000,
    };
    await saveToken(newToken);
    return newToken;
  } catch {
    return null;
  }
}

// Start the OAuth authorization code flow with PKCE
export async function startOAuthFlow(): Promise<{
  url: string;
  exchangeCode: (codeWithState: string) => Promise<TokenData>;
}> {
  const { verifier, challenge } = generatePKCE();

  const params = new URLSearchParams({
    code: "true",  // Required by claude.ai OAuth
    client_id: OAUTH_CONFIG.clientId,
    response_type: "code",
    redirect_uri: OAUTH_CONFIG.redirectUri,
    scope: OAUTH_CONFIG.scopes,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state: verifier,
  });

  const url = `${OAUTH_CONFIG.authorizeUrl}?${params}`;

  const exchangeCode = async (codeWithState: string): Promise<TokenData> => {
    // Parse code#state from the pasted value
    const parts = codeWithState.split("#");
    const code = parts[0];
    const state = parts[1];

    const resp = await fetch(OAUTH_CONFIG.tokenUrl, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        grant_type: "authorization_code",
        client_id: OAUTH_CONFIG.clientId,
        code,
        state,
        redirect_uri: OAUTH_CONFIG.redirectUri,
        code_verifier: verifier,
      }),
    });

    if (!resp.ok) {
      const text = await resp.text();
      throw new Error(`Token exchange failed (${resp.status}): ${text}`);
    }

    const data = (await resp.json()) as any;
    const token: TokenData = {
      access_token: data.access_token,
      refresh_token: data.refresh_token,
      expires_at: Date.now() + data.expires_in * 1000 - 5 * 60 * 1000,
    };
    await saveToken(token);
    return token;
  };

  return { url, exchangeCode };
}

// Get a valid access token (= API key for Claude Max).
// The access_token from claude.ai OAuth IS the API key.
// No create_api_key step needed.
export async function getAuthToken(): Promise<string | null> {
  const token = await loadToken();
  if (!token) return null;

  // Check expiry (with 5 min buffer already baked into expires_at)
  if (token.expires_at && Date.now() >= token.expires_at) {
    const refreshed = await refreshToken(token);
    if (refreshed) return refreshed.access_token;
    return null;
  }

  return token.access_token;
}

// TOKEN_FILE and TokenData are internal — not exported
