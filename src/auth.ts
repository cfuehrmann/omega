import { readFile, writeFile, mkdir } from "fs/promises";
import { join } from "path";
import { randomBytes, createHash } from "node:crypto";
import { createServer } from "node:http";

// OAuth configuration for Claude Max.
// redirect_uri must be localhost — platform.claude.com/oauth/code/callback is
// no longer accepted by Anthropic's token endpoint (returns 429 indefinitely).
const OAUTH_CONFIG = {
  clientId: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
  authorizeUrl: "https://claude.ai/oauth/authorize",
  tokenUrl: "https://platform.claude.com/v1/oauth/token",
  callbackPort: 53692,
  get redirectUri() { return `http://localhost:${this.callbackPort}/callback`; },
  scopes: "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers",
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
 * Spins up a local HTTP server on port 53692 to capture the redirect
 * automatically — no manual code pasting required.
 *
 * Returns:
 *   url      — open this in the browser
 *   complete — call immediately; resolves when the token is saved
 *   cancel   — call to abort (shuts down the local server)
 */
export async function startOAuthFlow(): Promise<{
  url: string;
  complete: () => Promise<void>;
  cancel: () => void;
}> {
  const { verifier, challenge } = generatePKCE();
  const redirectUri = OAUTH_CONFIG.redirectUri;

  const params = new URLSearchParams({
    code: "true",
    client_id: OAUTH_CONFIG.clientId,
    response_type: "code",
    redirect_uri: redirectUri,
    scope: OAUTH_CONFIG.scopes,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state: verifier,
  });

  const url = `${OAUTH_CONFIG.authorizeUrl}?${params}`;

  // Promise that resolves when the browser hits the callback URL.
  let resolveCallback!: (params: { code: string; state: string }) => void;
  let rejectCallback!: (err: Error) => void;
  const callbackPromise = new Promise<{ code: string; state: string }>((res, rej) => {
    resolveCallback = res;
    rejectCallback = rej;
  });

  const server = createServer((req, res) => {
    const reqUrl = new URL(req.url ?? "/", `http://localhost:${OAUTH_CONFIG.callbackPort}`);
    if (reqUrl.pathname !== "/callback") {
      res.writeHead(404); res.end(); return;
    }
    const code  = reqUrl.searchParams.get("code");
    const state = reqUrl.searchParams.get("state");
    const error = reqUrl.searchParams.get("error");
    if (error) {
      res.writeHead(400, { "Content-Type": "text/html" });
      res.end("<html><body><h2>Authorization failed. You can close this tab.</h2></body></html>");
      rejectCallback(new Error(`Authorization error: ${error}`));
      return;
    }
    if (!code || !state) {
      res.writeHead(400, { "Content-Type": "text/html" });
      res.end("<html><body><h2>Missing code or state. You can close this tab.</h2></body></html>");
      rejectCallback(new Error("Missing code or state in callback"));
      return;
    }
    res.writeHead(200, { "Content-Type": "text/html" });
    res.end("<html><body><h2>Authorized &#8212; you can close this tab.</h2></body></html>");
    resolveCallback({ code, state });
  });

  await new Promise<void>((res, rej) => {
    server.on("error", rej);
    server.listen(OAUTH_CONFIG.callbackPort, "127.0.0.1", res);
  });

  const cancel = () => {
    server.close();
    rejectCallback(new Error("OAuth cancelled"));
  };

  const complete = async (): Promise<void> => {
    const TIMEOUT_MS = 5 * 60 * 1000;
    const timer = setTimeout(() => {
      server.close();
      rejectCallback(new Error("OAuth timed out after 5 minutes"));
    }, TIMEOUT_MS);

    try {
      const { code, state } = await callbackPromise;
      clearTimeout(timer);
      server.close();

      const resp = await fetch(OAUTH_CONFIG.tokenUrl, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          grant_type: "authorization_code",
          client_id: OAUTH_CONFIG.clientId,
          code,
          state,
          redirect_uri: redirectUri,
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
    } catch (err) {
      clearTimeout(timer);
      server.close();
      throw err;
    }
  };

  return { url, complete, cancel };
}
