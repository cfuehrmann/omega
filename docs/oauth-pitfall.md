# OAuth Pitfall: Token vs API Key (Claude Max Billing)

## The Problem

When authenticating with Claude Max via OAuth, there are **two different
things** you can use to call the Anthropic API:

1. **OAuth access token** (Bearer token via `authToken` parameter)
2. **API key** created from that OAuth token

They both authenticate successfully. But **only the API key bills through
Claude Max**. The raw OAuth token bills per-token (pay-as-you-go), even
if the user has a Max subscription.

We discovered this the hard way: Omega was authenticating via OAuth and
showing "oauth (Claude Max)" in the status bar, but every API call was
billing per-token.

## How Claude Code Does It

By examining the Claude Code CLI (`@anthropic-ai/claude-code`), we found
the correct flow:

1. **OAuth PKCE flow** → get `access_token` + `refresh_token`
   - Endpoint: `https://platform.claude.com/oauth/authorize`
   - Token: `https://platform.claude.com/v1/oauth/token`
   - **Critical scope**: `org:create_api_key` (without this, step 2 fails)

2. **Exchange token for API key** → POST to create_api_key endpoint
   ```
   POST https://api.anthropic.com/api/oauth/claude_cli/create_api_key
   Authorization: Bearer <access_token>
   ```
   Response: `{ "raw_key": "sk-ant-..." }`

3. **Use the API key** for all API calls:
   ```typescript
   new Anthropic({ apiKey: raw_key })
   ```
   This key bills through Claude Max.

## The Wrong Way (what we did first)

```typescript
// ❌ WRONG: authenticates but bills per-token
new Anthropic({ authToken: oauthAccessToken })
```

## The Right Way (what we do now)

```typescript
// ✅ RIGHT: OAuth token → create API key → use API key
const resp = await fetch(
  "https://api.anthropic.com/api/oauth/claude_cli/create_api_key",
  { method: "POST", headers: { Authorization: `Bearer ${accessToken}` } }
);
const { raw_key } = await resp.json();
new Anthropic({ apiKey: raw_key });  // bills through Claude Max
```

## Required OAuth Scopes

Claude Code uses two scope sets:

- **For API key creation**: `org:create_api_key`, `user:profile`
- **For ongoing use**: `user:profile`, `user:inference`, `user:sessions:claude_code`, `user:mcp_servers`

Omega uses: `org:create_api_key`, `user:inference`, `user:profile`

The `org:create_api_key` scope is essential. Without it, the
`create_api_key` endpoint returns an error.

## Startup Verification

Having a key file is not proof of correct billing. On every startup,
`agent.ts init()` calls `verifyApiKey()` which:

1. Makes a free `POST /v1/messages/count_tokens` call (no tokens billed)
2. Inspects the `x-ratelimit-limit-requests` response header
3. Max accounts have limit > 200; pay-per-token Tier 1 has ~50

The UI shows the verified result:
- `✓ Authenticated: Claude Max (verified ✓)` — confirmed Max billing
- `⚠ Auth: api-key (pay-per-token ⚠)` — confirmed pay-per-token
- `⚠ Auth: ⚠ API key invalid: API usage limit reached` — key exhausted
- `⚠ Auth: oauth-derived (billing unknown)` — couldn't determine

This catches:
- Old keys from wrong OAuth scopes that bill per-token
- Exhausted pay-per-token keys (usage limit reached)
- Expired or revoked keys

## Files

- `src/auth.ts` — OAuth flow + API key creation + caching + verification
- `src/login.ts` — Interactive login script
- `src/agent.ts` — `init()` tries API key first, verifies billing via
  `verifyApiKey()`, falls back to env var, then raw OAuth token
- `~/.config/omega/oauth-token.json` — cached OAuth token
- `~/.config/omega/api-key` — cached API key (the one that actually matters)

## If Re-Authentication Is Needed

```bash
rm ~/.config/omega/api-key ~/.config/omega/oauth-token.json
cd ~/omega && bun run login
```
