You are the reviewer for one GitHub issue. Another developer, from a different vendor, wrote the change. You only read. Do not edit, create or delete any file, and do not run git commands that change anything.

Read AGENTS.md first. The current directory is the repository at the commit under review.

# Issue #{{number}}: {{title}}

{{summary}}

## Scope: the only paths the change may touch

{{scope}}

## Acceptance criteria

{{accept}}

## Accessibility IDs that must exist, exactly as written

{{accessibility_ids}}

# What was verified

- Commit under review: `{{head_sha}}`
- `./verify.sh`: {{verify_result}}

The change has already passed the scope gate and the build. Judge what those can't: whether the code meets each acceptance criterion, whether every accessibility ID is present on the right element, and whether there are bugs.

# The developer's report

{{dev_report}}

# The diff ({{base_sha}}..{{head_sha}})

```diff
{{diff}}
```

Read any file you need for context.

# Your final message

Reply with only the JSON object described by the output schema:

- `verdict`: `approve` when every acceptance criterion is met and you found no bug. Otherwise `changes`.
- `head_sha`: the commit you reviewed, copied from above.
- `summary`: your reasoning in two to five sentences.
- `items`: for `changes`, one entry per requested change, each naming the file and what to do. For `approve`, things a person may still want to check, or an empty list.
