---
name: factory-promote
description: Promote what factory merged into dev to main, through the promote pull request. Use when the person says "promote", "ship dev to main", "release what's on dev", or asks what would go to main, in a repository run by factory.
---

# Promote dev to main

Issues merge into `dev`, the default branch. `main` gets work only through the promote pull request, and merging that is the person's decision. Think of `main` as what would be released.

1. Find the `factory` program: `command -v factory`, or `~/code/task-manager/target/release/factory`.
2. Open the pull request, or refresh the open one, from this checkout. It uses only git and GitHub, so any machine with a checkout can run it:

   ```bash
   factory promote --repo-dir .
   ```

   It lists every change merged since the last promotion. Tell the person the link and how many changes it holds.
3. **The person merges it.** In GitHub, as a merge commit, ticking the box that bypasses the approval rule. Not a squash: a merge commit keeps `dev`'s history, so the next promotion lists only newer changes. Merge it yourself only when the person says so in this conversation, for this pull request:

   ```bash
   gh pr merge <number> --merge --admin --match-head-commit <head sha>
   ```

   Then check that `main` holds everything on `dev`: `gh api repos/<owner>/<name>/compare/main...dev --jq .ahead_by` answers 0.
4. A plan's parent issue is not closed by factory. When the person asks, close each one whose sub-issues are all closed: `gh api repos/<owner>/<name>/issues/<parent>/sub_issues --jq 'map(.state)'` shows them, then `gh issue close <parent> --reason completed`.
