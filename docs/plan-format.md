# Plan format

A plan is one YAML file in a project's `plans/` folder. It describes a parent issue and its child issues. Nothing here calls an LLM.

1. Open a pull request into `main` that adds `plans/<plan-id>.yaml`.
2. `plan-validate` checks it and comments the batches, the dependency graph and any errors. The check is required, so a plan that fails can't merge.
3. After the merge, `plan-create` creates the issues, the parent links and the blocked-by links, then comments the issue numbers on the pull request.

Check a plan locally before opening the pull request:

```bash
factory plan validate plans/
```

## Format

```yaml
plan: water-v1                  # plan ID. Must match the file name: plans/water-v1.yaml
title: Water tracker            # title of the parent issue
summary: Track daily water intake.   # optional
issues:
  - key: model                  # unique in this plan. Its global name is water-v1/model
    title: Water intake model and storage
    summary: Optional longer description.
    scope: [Sources/Model/]     # paths this issue may change
    accept:                     # acceptance criteria
      - Unit tests cover add, undo and the daily total
    labels: ["size:S"]

  - key: entry-ui
    title: Add-water screen
    scope: [Sources/UI/Entry/]
    accept:
      - Adding a glass updates the total on screen
    flows:                      # Maestro flows that must pass
      - {file: add_water.yaml, new: true}   # written for this issue, before development
    accessibility_ids: [water.entry.add, water.entry.total]
    depends_on: [model]
    labels: ["size:M", needs-sim]
```

| Field | Required | Meaning |
| --- | --- | --- |
| `plan` | yes | Plan ID: `a-z`, `0-9`, `-`, `_`, `.` |
| `title` | yes | Parent issue title |
| `issues[].key` | yes | `a-z`, `0-9`, `-`, `_` |
| `issues[].title` | yes | Issue title |
| `issues[].scope` | yes | Paths relative to the repo root. A directory covers everything below it. No patterns |
| `issues[].accept` | yes | Acceptance criteria, one per line |
| `issues[].flows` | no | A file name in `.maestro/`, or `{file, new: true}` for a flow this issue adds |
| `issues[].accessibility_ids` | no | IDs the flows use. Development must implement exactly these |
| `issues[].depends_on` | no | A key in this plan, `other-plan/key`, or an issue number written `"#42"` |
| `issues[].labels` | no | `size:S`, `size:M`, `needs-sim`, `needs-research`, `priority:high`, `priority:normal`, `priority:low` |

An unknown field is an error, so a typo like `depend_on` is caught.

## How to split work

- One child issue per change that can be tested and merged on its own.
- Steps that can't land alone stay as acceptance criteria inside one issue.
- Two issues that touch the same paths need an order. Add `depends_on` to one of them.

## What the validator checks

| Check | Catches |
| --- | --- |
| Required fields | An issue without acceptance criteria or scope |
| Unique keys | `model` defined twice |
| References | `depends_on: [modle]` |
| Cycles | `a → b → a`: neither can ever start |
| Scope overlap | Two unordered issues that both edit `Sources/UI/Shared/` |
| Folders people keep | A scope under `.claude/` (the skills people's sessions follow) or `.github/` (the workflows that check every pull request) |
| Flow files | A flow that doesn't exist, isn't marked new, and isn't created by a dependency |
| Labels | A label outside the allowed set |
| File name | `plans/a.yaml` holding `plan: b` |

It can't check a dependency you forgot when scopes don't overlap, whether an issue is the right size, or whether the acceptance criteria are good. Review the batch list for those.

## After the merge

Created issues are never edited by `factory`. Changing a merged plan file does not change its issues. To add work, add issues to the plan under new keys, or write a new plan.
