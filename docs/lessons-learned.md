# Lessons Learned

Read before integrating with external APIs or protocols.

## 1. Read working code first

Find a tool that already does what you need. Read its source — not docs,
not READMEs, not minified bundles. The OAuth fix took 10 minutes of
reading pi-ai's 100-line file after hours of guessing.

**Priority:** working open-source impl → SDK source → API docs → minified code.

## 2. Test in isolation before integrating

Write a 5-line script that makes one API call. Confirm it works before
wiring into the agent. Separates "can we auth?" from "does the agent work?"
and cuts debug cycles from minutes to seconds.

## 3. Don't trust error messages

"API usage limit reached" wasn't an exhausted key — it was a token from
the wrong OAuth endpoint (pay-per-token account). Error messages describe
symptoms, not causes. Question whether the credential itself is wrong.

## 4. Similar-looking endpoints aren't interchangeable

| Domain | Purpose |
|--------|---------|
| `claude.ai` | Max subscriptions |
| `console.anthropic.com` | Token exchange |
| `platform.claude.com` | Pay-per-token API |

Wrong domain for OAuth → silently different billing.

## 5. When impersonating, copy everything

OAuth tokens require Claude Code identity: beta headers, user-agent,
x-app, system prompt prefix. Missing any one → rejection. Copy all
details from the reference impl. Trim later, not before.

## 6. Red-green applies to infrastructure

We enforced red-green for code but not auth integration. A 5-line test
script after each auth change would have caught every mistake immediately.

## 7. Use `gh` for GitHub operations

The `gh` CLI is installed and authenticated as `cfuehrmann` with `repo` scope.
Use it instead of raw `git` for anything GitHub-specific:

```bash
gh repo view          # confirm remote
gh pr create          # open a PR
gh issue list         # browse issues
gh release create     # tag a release
gh auth status        # check auth
git push              # still use git for push/pull
```

Don't construct GitHub API URLs by hand or reach for `curl` — `gh` handles
auth, JSON parsing, and pagination automatically.

## Checklist for new API integrations

- [ ] Find and read a working implementation
- [ ] Note every URL, param, header (especially non-obvious ones)
- [ ] Write a standalone test script; confirm it works
- [ ] Only then build the full integration
