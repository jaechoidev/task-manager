---
name: factory-register-project
description: Register a project repository with the factory engine on this machine, so its issues labelled today get worked on. Use on the Mac Studio when the person says "register owner/name", "add the new project", or after the factory-new-project skill handed over.
---

# Register a project on the engine's machine

Run this on the Mac Studio, the one machine that watches for the `today` label, in a session started in `~/code/task-manager`. Never register a project on a second machine: two schedulers would both start the same issue.

## 0. Check where you are

- `gh api user --jq .login` answers `neostudiodot`, the agent account.
- `factory service status` exits 0: the scheduler and the API run. If not, stop and say so.
- The repository exists and has its skeleton on `dev`: `gh api repos/<owner>/<name> --jq .default_branch` answers `dev`.

## 1. Accept the invitation

```bash
gh api user/repository_invitations --jq '.[] | select(.repository.full_name == "<owner>/<name>") | .id'
```

```bash
gh api --method PATCH user/repository_invitations/<id>
```

No invitation listed is fine when `gh api repos/<owner>/<name> --jq .permissions.push` already answers `true`.

## 2. Clone, and give its commits the agent's name

```bash
git clone https://github.com/<owner>/<name> ~/code/<name>
```

The engine makes its worktrees from this checkout, and they take its git identity. The engine sets none of its own:

```bash
git -C ~/code/<name> config user.name neostudiodot
```

```bash
git -C ~/code/<name> config user.email "$(gh api user --jq '"\(.id)+\(.login)@users.noreply.github.com"')"
```

## 3. Register it

```bash
factory project add ~/code/<name>
```

It refuses a checkout whose `verify.sh` can't pass anything, and names what to fix. That fix belongs in the repository, from the session that made the skeleton, so report it and stop. A note about the commit name means step 2 didn't take.

## 4. Check it was taken in

The service reads the list again at each look, every 30 seconds or so. No restart is needed.

- `tail -5 ~/.factory/service/schedule.log` shows `now also <owner>/<name>`.
- `curl -s http://127.0.0.1:7878/v1/health` lists it in `repos`.
- `factory skills check <owner>/<name>` exits 0. If it doesn't, `factory skills update <owner>/<name>` writes them.

Then tell the person it's registered. It has no issues until a plan merges, and putting the `today` label on one starts the work. Don't label anything yourself.
