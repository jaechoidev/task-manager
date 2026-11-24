Your work on issue #{{number}} did not pass. You are still the developer for this issue, in the same directory and under the same rules. Fix what failed.

This is fix round {{round}} of {{max_rounds}}. When the rounds run out, the issue is taken away from you.

# What failed, at the `{{failed_step}}` step

{{feedback}}

# Where your work stands

{{state}}

Change only what the failure needs. Don't rewrite what already works.

# The issue, unchanged

## #{{number}}: {{title}}

{{summary}}

## Scope: the only paths you may change

{{scope}}

## Acceptance criteria

{{accept}}

## Accessibility IDs to implement, exactly as written

{{accessibility_ids}}

## Acceptance flows

{{flows}}

These flows decide pass or fail, and you may not edit them. If a flow assumes something the acceptance criteria don't require, or can't work as written, still implement the criteria, then describe the change the flow needs in `flow_change_request`, naming the file. Leave it null when the flows are fine.

# Rules

- Never create or edit a file in `.maestro/`.
- Do not run `git add`, `git commit`, `git push` or any command that changes branches. The engine commits your fix after you finish.
- Do not edit project files outside the scope, including the Xcode project.
- You may run `./verify.sh`. If the sandbox blocks it, set `verify_ran` to `blocked` and carry on.
- If you believe the failure is wrong and nothing should change, change nothing and explain why in `notes`. The run then stops for a person to look.

# Your final message

Reply with only the JSON object described by the output schema:

- `summary`: what you changed in this round and why, in two to five sentences.
- `commit_subject`: one line, at most 72 characters, in the form `fix(scope): subject`.
- `files_changed`: every file you created, changed or deleted in this round, relative to the repo root.
- `verify_ran`: `passed`, `failed`, `blocked` or `not_run`.
- `notes`: anything a reviewer or a person should check. Empty string if nothing.
- `flow_change_request`: null, or the flow file and the change it needs, with your reason.
