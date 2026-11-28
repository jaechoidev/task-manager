---
name: factory-new-project
description: Create a new project repository run by factory, from the app template, up to the point where the Mac Studio can register it. Use when the person says "set up a new factory project", "new project called X", or "make a repo for factory". Runs on the MacBook, as the account that will own the repository.
---

# Set up a new factory project

The engine already runs on the Mac Studio as `neostudiodot`. This skill makes the repository and its app skeleton on the MacBook, then hands over to the Mac Studio, where the `factory-register-project` skill registers it. Stop and ask at every step marked **Ask**. The person's word in this conversation is the only approval that counts.

## 0. Check where you are

- `gh api user --jq .login` names the account that will own the repository, `jaechoidev`. An agent account can't do this: settings checked here are shown only to an admin.
- Find `factory`: `command -v factory`, or `~/code/task-manager/target/release/factory`. If neither exists, build it with `cargo build --release` in `~/code/task-manager`.
- Agree on the name with the person: lowercase, words joined by `-`, for example `notes-app`.

## 1. Create the repository. **Ask** first

```bash
factory repo init jaechoidev/<name> --template jaechoidev/app-template --collaborator neostudiodot
```

It creates the private repository from the template, makes `dev` the default branch, sets the rulesets, labels and Actions settings, writes the factory skills to `dev`, and ends with a preflight. Two warnings are expected at this point: `neostudiodot` hasn't accepted the invitation, and the review secret is missing.

## 2. The review secret. The person does this

Only the person may handle the token. Never ask for it, see it or type it. Give them these two commands, in this order, and wait until they say it's done:

```bash
claude setup-token
```

```bash
gh secret set CLAUDE_CODE_OAUTH_TOKEN --repo jaechoidev/<name>
```

## 3. The app skeleton

The template has the workflows, `verify.sh`, `AGENTS.md` and `plans/`, but no app. `./verify.sh` must pass on `dev` before any issue runs, because every issue's verify builds the app and runs every flow. Use `~/code/counter-trial` as the worked example of the shape:

- An Xcode project at the root (`<App>.xcodeproj`), a SwiftUI app, bundle ID `com.example.<name without dashes>`.
- Code under `Sources/`: `Sources/App/` for the app, one folder per screen under `Sources/UI/`, so plans can give each issue its own scope.
- A unit-test target `<App>Tests` whose synchronized folder is `Tests/`, with at least one test, so the unit-test step in `verify.sh` runs from day one. Test folders mirror `Sources/`, as in `Tests/Model/` for `Sources/Model/`. counter-trial has none, so copy the shape from my-favorite-thing's commit `cd22c78` (`gh api repos/jaechoidev/my-favorite-thing/commits/cd22c78 -H "Accept: application/vnd.github.diff"`), which writes it by hand into an objectVersion 77 project:
  - a `PBXFileSystemSynchronizedRootGroup` with path `Tests` in the main group, and an `<App>Tests.xctest` product reference in the Products group
  - a `PBXNativeTarget` of type `com.apple.product-type.bundle.unit-test` with empty Sources, Frameworks and Resources phases, the `Tests` group in `fileSystemSynchronizedGroups`, and a `PBXTargetDependency` on the app through a `PBXContainerItemProxy`
  - `TestTargetID` set to the app target in `TargetAttributes`, and the new target in the project's `targets`
  - build settings `TEST_HOST = "$(BUILT_PRODUCTS_DIR)/<App>.app/$(BUNDLE_EXECUTABLE_FOLDER_PATH)/<App>"`, `BUNDLE_LOADER = "$(TEST_HOST)"`, `GENERATE_INFOPLIST_FILE = YES` and `PRODUCT_BUNDLE_IDENTIFIER = com.example.<name without dashes>tests`
  - a `TestAction` in the shared scheme whose testable has `parallelizable = "NO"`, because parallel testing clones the simulator and the engine gives each verify one
- The app hosts the tests, so launching it never asks for a permission such as photos or camera. Ask on a tap, or the alert stops the tests and the flows.
- A root accessibility ID on each screen, as in `counter.screen.root`, following the format in `AGENTS.md`.
- `.maestro/launch.yaml`: the app launches and each screen can be reached. Its `appId` is the bundle ID. Remove `.maestro/sample.yaml` if it doesn't fit the app.
- In `verify.sh`, set `SCHEME` and `PROJECT`, for example `SCHEME="${SCHEME:-NotesApp}"` and `PROJECT="${PROJECT:-NotesApp.xcodeproj}"`.

```bash
git clone https://github.com/jaechoidev/<name> ~/code/<name>
```

Write the skeleton there, on `dev`, then run `./verify.sh` until it exits 0. **Ask** before pushing it straight to `dev`:

```bash
git -C ~/code/<name> add -A
git -C ~/code/<name> commit -m "chore: add the app skeleton"
git -C ~/code/<name> push origin dev
```

Then check everything again. Only the invitation may still warn:

```bash
factory repo preflight jaechoidev/<name> --collaborator neostudiodot
```

## 4. Hand over to the Mac Studio

Tell the person the next step happens on the Mac Studio: in its session, started in `~/code/task-manager`, say "register jaechoidev/<name>". The `factory-register-project` skill accepts the invitation, clones, sets the commit name and registers the project. The running service takes it in without a restart. Send that session a message yourself only if the person asks you to.

## 5. Watching from the MacBook

If `~/.zshrc` has no `fv` alias, offer to add it. **Ask** before editing the file. It opens the tunnel to the Mac Studio's API if it isn't open, then the app:

```bash
alias fv='pgrep -qf "7878:127.0.0.1:7878" || ssh -fN -L 7878:127.0.0.1:7878 jae@neo-studio.local; open ~/code/task-manager/app/build/Factory.app'
```

## What the person does next

Plans are written in a session started in `~/code/<name>`, which has the project's `factory-plan` skill. Merging a plan into `main` and putting the `today` label on issues stay theirs.
