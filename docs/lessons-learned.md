# Lessons Learned

Hard-won lessons from building Omega. Read this before starting a new
feature, especially one involving external APIs or protocols.

## 1. Read Working Code Before Writing Your Own

**The OAuth disaster:** We spent multiple sessions guessing at Anthropic's
OAuth flow — wrong URLs, wrong parameters, wrong headers, wrong auth
method. The correct implementation was sitting in `pi-ai`'s
`dist/utils/oauth/anthropic.js` — 100 lines, fully readable, every
detail spelled out. We could have gotten it right on the first try.

**Rule: When integrating with an API that another tool already uses
correctly, read that tool's source code FIRST.** Don't read docs alone
(they may be incomplete or misleading). Don't read minified/bundled code
when there's a readable version available. Don't guess and iterate.

### Where to look (priority order)

1. **Working open-source implementations** — Find code that does exactly
   what you need. Read it. Not the README, not the docs — the actual
   source. For OAuth flows, API integrations, protocol implementations.

2. **Official SDKs** — The SDK source often reveals undocumented
   parameters, headers, and behaviors. Read the actual HTTP requests
   being made, not just the public API.

3. **API documentation** — Useful for understanding the model, but
   often incomplete on exact endpoints, headers, and edge cases.

4. **Minified/bundled code** — Last resort. Hard to read, multiple code
   paths, easy to follow the wrong branch.

### Checklist for API integration

Before writing any code:

- [ ] Find a working implementation (open source preferred)
- [ ] Read its source — identify exact URLs, params, headers
- [ ] Note anything non-obvious (extra params like `code=true`,
      special headers, specific URL domains)
- [ ] List every endpoint and its exact URL
- [ ] Identify the auth method (apiKey vs Bearer vs custom header)
- [ ] Write a minimal test script that makes one API call
- [ ] Confirm it works before building the full integration

## 2. Test the Integration Immediately

**What went wrong:** We implemented the full OAuth flow, wired it into
the agent, and only discovered it was broken when running `bun start`.
Each fix required another full login cycle (browser, paste code, restart).

**Rule: Write a standalone test script that exercises the API call in
isolation BEFORE integrating.** Something like:

```typescript
// test-auth.ts — run once to verify the API accepts our credentials
const client = new Anthropic({ authToken: token, apiKey: null, ... });
const resp = await client.messages.create({
  model: "claude-sonnet-4-20250514",
  max_tokens: 10,
  messages: [{ role: "user", content: "Say hi" }],
});
console.log(resp.content[0]);
```

This separates "can we authenticate?" from "does the agent work?" and
cuts the debug loop from minutes to seconds.

## 3. Don't Trust Error Messages at Face Value

**What happened:** "API usage limit reached" made us think the key was
exhausted. But the real problem was that the token was created via the
wrong OAuth endpoint (platform.claude.com instead of claude.ai), so it
was a pay-per-token key on an account that had hit its limit.

**Rule: When an API returns an error, consider whether the credential
itself might be wrong, not just expired/exhausted.** The error message
describes the symptom, not the cause.

## 4. Understand the Difference Between Similar-Looking Endpoints

Anthropic has multiple domains that look interchangeable but aren't:

| Domain | Purpose |
|--------|---------|
| `claude.ai` | Consumer product (Pro/Max subscriptions) |
| `console.anthropic.com` | Developer console + token exchange |
| `platform.claude.com` | API platform (pay-per-token billing) |
| `api.anthropic.com` | API endpoint for messages |

Using the wrong domain for OAuth authorization creates tokens with
different billing — the API doesn't tell you which billing type your
token has. It just works (or hits limits) silently.

**Rule: When an API has multiple domains, understand what each one is
for. Don't assume they're interchangeable.**

## 5. When Impersonating a Client, Get ALL the Details

OAuth tokens from claude.ai require Claude Code identity:
- Specific beta headers (`claude-code-20250219,oauth-2025-04-20`)
- Specific user-agent (`claude-cli/2.1.2`)
- Specific app header (`x-app: cli`)
- Specific system prompt prefix

Missing ANY of these causes auth failures. When you need to match an
existing client's behavior, match it exactly — don't cherry-pick.

**Rule: Copy all headers, all parameters, all conventions from the
reference implementation. Trim later once you know what's actually
required, not before.**

## 6. Provider Auth Will Vary — Plan for It

When we add other providers (OpenAI, Google, etc.), each will have its
own OAuth quirks, billing models, and undocumented requirements. The
current lesson applies broadly:

- Read a working implementation first
- Test in isolation before integrating
- Don't assume standard OAuth is actually standard
- Check for provider-specific headers and parameters

Future providers should each get their own `docs/auth-<provider>.md`
documenting the exact flow, with references to the source code we
reverse-engineered from.

## 7. Red-Green Still Applies to Infrastructure

We enforced red-green for code bugs but not for infrastructure
integration (auth, API calls). The principle still applies:

1. **Red**: Write a test that calls the API and expects a valid response
2. **Confirm it fails** (wrong auth, wrong endpoint, etc.)
3. **Green**: Fix the auth until the test passes

We skipped this and went straight to "implement → run → see what breaks."
A 5-line test script after each auth change would have saved hours.
