---
name: factory-update-engine
description: Update the factory engine on the machine that runs it to the latest task-manager main, restart its service, and refresh the skills of every registered project. Use on the Mac Studio when the person says "update the engine", "update factory", or "pull the latest task-manager".
---

# Update the engine

Run this on the Mac Studio in a session started in `~/code/task-manager`. Restarting the service needs the person's word in this conversation, every time.

## 1. See what would be cut off

```bash
curl -s http://127.0.0.1:7878/v1/schedule
```

Reinstalling stops both agents. An issue at work is cut off and picks up at the step it was on the next time it runs. If anything runs, say which issues, and let the person choose between waiting and going ahead.

## 2. Build the new version

```bash
git -C ~/code/task-manager pull --ff-only
```

```bash
cargo install --path ~/code/task-manager
```

This replaces `~/.cargo/bin/factory`. The agents keep running the old program until they start again.

## 3. Restart the service. **Ask** first

Run it from a login shell whose `PATH` finds `git`, `gh`, `claude`, `codex` and the Xcode tools. The agents get that `PATH` and nothing else. Keep the flags it runs with now: they are the arguments after `schedule --watch` in `~/Library/LaunchAgents/dev.factory.schedule.plist`, such as `--parallel 2`, and a `--bind` in `dev.factory.serve.plist`. With none:

```bash
factory service install
```

```bash
factory service status
```

`status` exits 0 only when both agents run and the API answers as its own process. Report the version and process id `curl -s http://127.0.0.1:7878/v1/health` gives.

## 4. Refresh each project's skills

```bash
factory project list
```

For each project, with its `owner/name`:

```bash
factory skills update <owner>/<name>
```

It commits the skills of the new version to the project's default branch through GitHub, as this machine's `gh` account, or says they already match. Report what each one did.
