---
name: factory-status
description: See what factory is doing on this repository's issues, what is blocked and why, and retry or drop a stuck issue. Use when the person asks "what's running", "what's blocked", "why did issue N fail", "retry N" or "drop N" in a repository run by factory.
---

# What factory is doing, and stuck issues

The engine runs on one machine, the Mac Studio. Its API answers on `http://127.0.0.1:7878` there. From the MacBook it is reached through an SSH tunnel to the same address: `ssh -fN -L 7878:127.0.0.1:7878 jae@neo-studio.local`, or the person's `fv` alias, which also opens the app.

## Reading

Check the API first. Its `repos` must name this repository (`gh repo view --json nameWithOwner --jq .nameWithOwner`):

```bash
curl -s -m 3 http://127.0.0.1:7878/v1/health
```

No answer means no tunnel or no engine. Say which one and stop. Don't start `factory serve` yourself.

| Question | Call |
| --- | --- |
| What runs now, what waits and on what, who holds each slot | `curl -s http://127.0.0.1:7878/v1/schedule` |
| Every issue the engine knows, with its stage and state | `curl -s http://127.0.0.1:7878/v1/issues` |
| What waits for a person | `curl -s http://127.0.0.1:7878/v1/blocked` |
| One issue in full | `curl -s http://127.0.0.1:7878/v1/issues/<owner>/<name>/<number>` |
| An issue's run log | `curl -s http://127.0.0.1:7878/v1/issues/<owner>/<name>/<number>/log` |

Answer in plain words: the issue, the step it is at or failed at, and why. A blocked issue's `judgment` says what the stuck-issue judgment decided. Its pull request is `pr_url`.

## Retry or drop

Both change things on GitHub, so do either only when the person says so in this conversation, naming the issue.

- **Retry** closes the issue's pull request, deletes the branch its last attempt left, and runs it again from nothing. Worth it once the cause is fixed: an issue whose text was wrong, a flaky simulator, a provider that was down.
- **Drop** closes the pull request, deletes the branch and worktree, and takes off the labels that would pick it again. The issue stays open for a person.

```bash
curl -s -X POST http://127.0.0.1:7878/v1/issues/<owner>/<name>/<number>/retry
```

```bash
curl -s -X POST http://127.0.0.1:7878/v1/issues/<owner>/<name>/<number>/drop
```

A retry answers at once and runs on its own: say where its log is, then read `/v1/schedule` or the issue again later to report how it went. Never relabel or edit an issue to get around a block. Say what you would change and let the person decide.
