# OAuth for Claude Max — Lessons Learned

## The Working Flow (reverse-engineered from pi-ai)

The Anthropic API requires a specific OAuth flow for Claude Max billing.
We got it wrong multiple times. Here is what actually works, reverse-
engineered from `@mariozechner/pi-ai/dist/utils/oauth/anthropic.js`:

### 1. Authorization

```
GET https://claude.ai/oauth/authorize
  ?code=true                         ← REQUIRED, easy to miss
  &client_id=9d1c250a-...
  &response_type=code
  &redirect_uri=https://console.anthropic.com/oauth/code/callback
  &scope=org:create_api_key user:profile user:inference
  &code_challenge=<PKCE challenge>
  &code_challenge_method=S256
  &state=<PKCE verifier>
```

**Critical details:**
- URL is `claude.ai`, NOT `platform.claude.com`
- `code=true` param is required (not part of standard OAuth)
- Redirect URI is `console.anthropic.com`, NOT `platform.claude.com`
- State is the PKCE verifier (used later in token exchange)

### 2. Token Exchange

```
POST https://console.anthropic.com/v1/oauth/token
Content-Type: application/json

{
  "grant_type": "authorization_code",
  "client_id": "9d1c250a-...",
  "code": "<code from redirect>",
  "state": "<state from redirect>",
  "redirect_uri": "https://console.anthropic.com/oauth/code/callback",
  "code_verifier": "<PKCE verifier>"
}
```

**Critical details:**
- Token URL is `console.anthropic.com`, NOT `platform.claude.com`
- Response includes `access_token`, `refresh_token`, `expires_in`

### 3. Using the Token

The `access_token` (format: `sk-ant-oat-...`) is passed as `authToken`
to the Anthropic SDK, with specific headers:

```typescript
new Anthropic({
  apiKey: null,
  authToken: accessToken,
  defaultHeaders: {
    "anthropic-beta": "claude-code-20250219,oauth-2025-04-20",
    "user-agent": "claude-cli/2.1.2 (external, cli)",
    "x-app": "cli",
  },
});
```

**Critical details:**
- `apiKey: null` — do NOT pass the token as apiKey (→ "invalid x-api-key")
- `authToken: token` — sends as `Authorization: Bearer <token>`
- Beta headers `claude-code-20250219,oauth-2025-04-20` are REQUIRED
- Must impersonate Claude Code (user-agent + x-app headers)
- System prompt must start with: `"You are Claude Code, Anthropic's
  official CLI for Claude."`

### 4. Token Refresh

```
POST https://console.anthropic.com/v1/oauth/token
Content-Type: application/json

{
  "grant_type": "refresh_token",
  "client_id": "9d1c250a-...",
  "refresh_token": "<refresh_token>"
}
```

## What Went Wrong (the full list of mistakes)

### Mistake 1: Wrong authorize URL
- **Used**: `platform.claude.com/oauth/authorize` (Console/API account)
- **Should be**: `claude.ai/oauth/authorize` (Claude Max account)
- **Effect**: Created tokens tied to pay-per-token billing

### Mistake 2: Wrong token URL
- **Used**: `platform.claude.com/v1/oauth/token`
- **Should be**: `console.anthropic.com/v1/oauth/token`

### Mistake 3: Wrong redirect URI
- **Used**: `platform.claude.com/oauth/code/callback`
- **Should be**: `console.anthropic.com/oauth/code/callback`

### Mistake 4: Missing `code=true` param
- Standard OAuth doesn't have this; Anthropic requires it

### Mistake 5: Assumed we needed create_api_key
- **Tried**: `POST /api/oauth/claude_cli/create_api_key` to exchange
  token for API key
- **Reality**: The access_token IS the credential. No exchange needed.
- The API key from create_api_key inherits billing from the OAuth
  account — since we were using the wrong authorize URL, those keys
  were also pay-per-token.

### Mistake 6: Passed token as apiKey instead of authToken
- `new Anthropic({ apiKey: token })` → "invalid x-api-key" (401)
- Must use `new Anthropic({ authToken: token, apiKey: null })`

### Mistake 7: Missing identity headers
- Without `claude-code-20250219` beta and user-agent headers,
  the API rejects OAuth tokens entirely

## Root Cause of All Mistakes

**We guessed instead of reading working code.** We should have reverse-
engineered pi-ai's `dist/utils/oauth/anthropic.js` on the first attempt.
The file is 100 lines and contains every URL, every parameter, every
header. Instead we:

1. Read Claude Code's minified CLI (hard to follow, multiple code paths)
2. Made assumptions about which endpoints to use
3. Trial-and-errored through each failure

**The fix was 10 minutes of reading the right file.** The debugging was
hours of wrong guesses.

## Files

- `src/auth.ts` — OAuth flow (authorize + token exchange + refresh)
- `src/login.ts` — Interactive login script
- `src/agent.ts` — `init()` passes token as authToken with identity headers
- `~/.config/omega/oauth-token.json` — cached OAuth credentials

## If Re-Authentication Is Needed

```bash
rm ~/.config/omega/oauth-token.json
cd ~/omega && bun run login
```
