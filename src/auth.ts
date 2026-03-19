import { readFile, writeFile, mkdir } from "fs/promises";
import { join } from "path";

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
        scope: OAUTH_CONFIG.scopes,
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

/**
 * Force a token refresh regardless of expiry.
 * Call this when a 401 "OAuth token has expired" error is received mid-session.
 * Returns the new access token, or null if refresh failed (no refresh_token, or
 * the refresh itself was rejected).
 */
export async function forceRefreshToken(): Promise<string | null> {
  const token = await loadToken();
  if (!token) return null;
  const refreshed = await refreshToken(token);
  return refreshed ? refreshed.access_token : null;
}

// TOKEN_FILE and TokenData are internal — not exported
