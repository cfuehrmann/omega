# Anthropic OAuth for Claude Max

Reverse-engineered from `@mariozechner/pi-ai/dist/utils/oauth/anthropic.js`.

## Endpoints

| Step | URL | ⚠ NOT |
|------|-----|-------|
| Authorize | `claude.ai/oauth/authorize` | `platform.claude.com` |
| Token exchange | `console.anthropic.com/v1/oauth/token` | `platform.claude.com` |
| Redirect URI | `console.anthropic.com/oauth/code/callback` | `platform.claude.com` |

Wrong domain → tokens billed pay-per-token instead of Max.

## Authorization params

```
code=true  client_id=9d1c250a-...  response_type=code
redirect_uri=https://console.anthropic.com/oauth/code/callback
scope=org:create_api_key user:profile user:inference
code_challenge=<PKCE>  code_challenge_method=S256  state=<verifier>
```

`code=true` is non-standard but required.

## SDK usage

```typescript
new Anthropic({
  apiKey: null,           // NOT as apiKey (→ 401)
  authToken: accessToken, // Bearer auth
  defaultHeaders: {
    "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
    "user-agent": "claude-cli/2.1.2 (external, cli)",
    "x-app": "cli",
  },
});
```

All headers are required. Missing any one → auth rejection.

## Token refresh

```
POST console.anthropic.com/v1/oauth/token
{ "grant_type": "refresh_token", "client_id": "9d1c250a-...", "refresh_token": "..." }
```

## Files

- `src/auth.ts` — OAuth flow (authorize + exchange + refresh)
- `src/agent.ts` — `init()` passes token as authToken with headers
- `~/.config/omega/oauth-token.json` — cached credentials

## Re-authenticate

```bash
rm ~/.config/omega/oauth-token.json && cd ~/omega && bun run login
```
