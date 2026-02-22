# OpenAI Codex fallback (5.2)

Omega can fall back to OpenAI Codex when Anthropic rate limits (HTTP 429).

## Auth

Set an OpenAI API key:

```bash
export OPENAI_API_KEY="sk-..."
```

Optional base URL (if using a proxy or gateway):

```bash
export OPENAI_BASE_URL="https://api.openai.com/v1"
```

## Model

Config in `src/config.ts`:

```ts
fallbackModel: "gpt-5.2-codex"
```

## Behaviour

- Anthropic is primary.
- If an Anthropic call fails with rate limit (429), Omega logs a fallback
  event and replays the request against OpenAI.
- The fallback call uses OpenAI Chat Completions with tool support and
  converts responses back into Anthropic-style content blocks.

## Notes

- Pricing for Codex is not wired into `PRICING` yet (set to 0 by default).
- If `OPENAI_API_KEY` is missing, fallback is disabled.
