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
- The fallback call uses the OpenAI Responses API (`/v1/responses`). Input
  messages are sent as string `content` (not `input_text` blocks).
- Anthropic `tool_use` blocks are mapped to `function_call` inputs; tool
  results map to `function_call_output`. If tool_result appears before the
  tool_use in history, Omega injects the missing `function_call` before the
  output to satisfy the Responses API.
- Once a rate limit triggers, fallback stays active for the rest of the
  runtime (no automatic switch-back).

## Notes

- Pricing for Codex is not wired into `PRICING` yet (set to 0 by default).
- If `OPENAI_API_KEY` is missing, fallback is disabled.
