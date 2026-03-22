import { readFile, writeFile, mkdir } from "fs/promises";
import { join } from "path";
import { randomBytes, createHash } from "node:crypto";

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
const AUTH_CONFIG_FILE = join(CONFIG_DIR, "config.json");

// In test mode, config file operations are no-ops / always-null.
const IS_TEST = process.env.OMEGA_TEST === "1";

// ---------------------------------------------------------------------------
// Auth mode config
// ---------------------------------------------------------------------------

export type AuthMode = "claude-max" | "api-key";

export interface AuthConfig {
  authMode: AuthMode;
}

export async function readAuthConfig(): Promise<AuthConfig | null> {
  if (IS_TEST) return null;
  try {
    const data = await readFile(AUTH_CONFIG_FILE, "utf-8");
    const parsed = JSON.parse(data);
    if (parsed.authMode === "claude-max" || parsed.authMode === "api-key") {
      return { authMode: parsed.authMode };
    }
    return null;
  } catch {
    return null;
  }
}

export async function writeAuthConfig(mode: AuthMode): Promise<void> {
  if (IS_TEST) return;
  await ensureConfigDir();
  await writeFile(AUTH_CONFIG_FILE, JSON.stringify({ authMode: mode }, null, 2), "utf-8");
}

// ---------------------------------------------------------------------------
// Token storage helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Token refresh
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Auth token result
// ---------------------------------------------------------------------------

/**
 * Richer result type for getAuthToken() — distinguishes the three cases so
 * callers can give actionable errors rather than silently falling back.
 *
 *   ok            — valid token returned
 *   no_token      — no oauth-token.json file (never logged in)
 *   refresh_failed — file exists but the refresh token is expired/revoked;
 *                    re-login required
 */
export type AuthTokenResult =
  | { kind: "ok"; token: string }
  | { kind: "no_token" }
  | { kind: "refresh_failed" };

/**
 * Load and (if necessary) refresh the OAuth access token.
 * Returns a typed result so callers can distinguish "never logged in" from
 * "logged in but the refresh token has expired" — the latter requires
 * explicit re-login, not a silent fallback.
 */
export async function getAuthToken(): Promise<AuthTokenResult> {
  const token = await loadToken();
  if (!token) return { kind: "no_token" };

  // Token is still valid (with 5 min buffer already baked in).
  if (!token.expires_at || Date.now() < token.expires_at) {
    return { kind: "ok", token: token.access_token };
  }

  // Access token expired — attempt refresh.
  const refreshed = await refreshToken(token);
  if (refreshed) return { kind: "ok", token: refreshed.access_token };

  // Refresh token is dead too — user must re-login.
  return { kind: "refresh_failed" };
}

/**
 * Force a token refresh regardless of expiry.
 * Call this when a 401 "OAuth token has expired" error is received mid-session.
 * Returns the new access token, or null if refresh failed.
 */
export async function forceRefreshToken(): Promise<string | null> {
  const token = await loadToken();
  if (!token) return null;
  const refreshed = await refreshToken(token);
  return refreshed ? refreshed.access_token : null;
}

// ---------------------------------------------------------------------------
// OAuth PKCE authorization flow
// ---------------------------------------------------------------------------

function generatePKCE(): { verifier: string; challenge: string } {
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  return { verifier, challenge };
}

/**
 * Start the OAuth authorization code flow with PKCE.
 *
 * Returns the authorization URL and an `exchangeCode` function to call
 * once the user has authorized and pasted back the `code#state` string
 * from the redirect URL.
 */
export async function startOAuthFlow(): Promise<{
  url: string;
  exchangeCode: (codeWithState: string) => Promise<void>;
}> {
  const { verifier, challenge } = generatePKCE();

  const params = new URLSearchParams({
    code: "true",
    client_id: OAUTH_CONFIG.clientId,
    response_type: "code",
    redirect_uri: OAUTH_CONFIG.redirectUri,
    scope: OAUTH_CONFIG.scopes,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state: verifier,
  });

  const url = `${OAUTH_CONFIG.authorizeUrl}?${params}`;

  const exchangeCode = async (codeWithState: string): Promise<void> => {
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
  };

  return { url, exchangeCode };
}
