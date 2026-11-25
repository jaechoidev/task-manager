# Findings: choosing models in the app (2026-11-25)

The question was "can't I change which model to use in the UI?" The answer was no: the models were only in `providers.yaml`. This adds a Providers screen that edits that file through the engine.

## Result

| | |
| --- | --- |
| Built | `GET` and `POST /v1/providers`, `POST /v1/providers/check`, an editor for the file that keeps its comments, and the Providers screen |
| Tested with stand-ins | 135 unit tests and 55 scenarios in the engine, 17 tests in the app |
| Run for real | In the real window against a scratch copy of the providers file: reordering a job's list, adding someone to a job, and saving. A model change through the API with a made-up model name, whose check then ran for real and failed |
| Not run for real | A model change that passes its check. A save against the real providers file |

## What the screen does

- **Jobs.** For each of research, acceptance, develop, review, summary and judgment: who is tried, in order, with buttons to move an entry up or down, take it out, or add one. A job with no list says that the workflow's own vendor does it.
- **Providers.** Each tier with the model it names in a field, its family, and what its check found. A row to add a tier.
- **Save and check.** One request with only what changed. A tier whose model changed is checked at once.

## How the file is changed

- **In place.** The file has comments, and they are the user's. A tier's line is rewritten on its own line, keeping a comment after the value. The `roles:` block is replaced as a block. Nothing else is touched.
- **Read back.** After the edit the text is parsed again and compared with what was asked for. A file laid out in a way the editor can't follow (tiers on one line, for one) is left alone with a message to edit it by hand.
- **The old file is kept** beside it as `providers.yaml.bak`.
- **The engine's rules decide.** A change is applied to the parsed file first, and everything `factory providers` would call a problem refuses it, with every reason.

## What the real window showed

Against a scratch engine holding a copy of the real file:

- Moving `chatgpt sol` down in develop and adding `claude standard` to research, then "Save and check", changed exactly those lines. The diff of the file was two lines moved and two added, and every comment stayed.
- A model changed to `gpt-0-test` showed as "not usable until it passes its check" in the develop list at once.
- Its check ran for real. Codex refused the model, and the tier's line turned red with the reason.

## Problems hit

1. **The check hid Codex's own words.** Codex said "The 'gpt-0-test' model is not supported when using Codex with a ChatGPT account." The engine reported that the final message was not JSON. It now reports what Codex said, in a check and in a run.
2. **The model fields could not be typed into by the test.** The window was driven in the background through the system's accessibility. The sidebar's filter field takes text that way. The model fields do not, and the cause was not found. Moving the label onto the field itself did not change it. The owner then typed into one by hand, and it works: the fault is in how the test reaches the field, not in the field.
3. **Text meant for a model field went into the filter.** A side effect of the above, on the scratch app only. Cleared.
4. **The row for adding someone stayed open after a save.** It closes now.

## Open items

- **Why the test could not type into a model field** is still not known. By hand it works.
- **Tiers can be added, not renamed or removed.** A rename would have to follow into every list.
- **Providers can't be added.** A provider has a login, which is a file or a folder on disk, not something to type into an app.
- **A model name is free text.** Nothing lists the models an account has. A wrong name is found by the check, which costs one refused call.
- **Changing a model takes the tier out of use until its check passes.** If it is the only entry of a job's list, that job has nobody until then. The screen says so on the entry, and does not stop the save.
- **The roles block is written in one fixed style.** Comments inside it are lost. The file had none.
