# Factory, the macOS app

A window and a menu bar item onto the engine. It is a client of `factory serve` and of nothing else: every number it shows comes from the engine's API, and its two buttons call the same commands the CLI has.

## What it shows

| Part | What is there |
| --- | --- |
| Toolbar | A project picker (all projects, or one), whether the engine answers and how long ago, and a button to ask again now |
| Sidebar | A filter, the three places to go (Schedule, Workflow, Usage), and every issue the engine has run, grouped: needs you, running and waiting, pull request open, done. Each issue has a mark for its state. With more than one repository, each row says which |
| Issue | Its state, the pass it is on and its pull request. A strip of the workflow's steps with how each went. The summary written for a person. A "to look at" list you can tick off (kept on this machine). Every step, grouped by pass, with who answered on which model and a bar for its time. The end of the newest output, opened while the issue is at work |
| Needs you | For a stuck issue: what failed, what the judgment said, and two buttons, "Start over" and "Take off the queue" |
| Schedule | How many places are in use. Per project: the issues holding a place with the step each is at, the parked ones with their time, and the ones not started with what each waits on. Each pool's slots with who holds them. Who is set aside for a usage limit, when anyone is: that explains a wait |
| Workflow | The repository's steps as a graph, with a dashed red line from each step to where its failure goes, and the limits it runs under. Read only |
| Edit workflow | Reached from Workflow. The workflow file's text, checked by the engine as you type. The graph of the draft, with the steps the engine refuses marked. The problems with their line numbers, the modules a step may name, and a form that scaffolds a new module. "Open a pull request" is how a change is saved |
| Usage | Quota first: each provider's plan with a bar per window, and a button that reads every plan again (one small call each for Claude and ChatGPT). Then calls, tokens and usage limits hit per vendor, with a bar for the tokens |
| Menu bar | How many issues need a person, what is running, and how many places are in use |

The look follows the redesign canvas. It is drawn for a light window, so the app is light whatever the system is set to.

Review, diffs and merge approval stay on GitHub.

## Run it

Start the engine's API, with a checkout of each repository it may act on:

```bash
factory serve --repo-dir ~/code/todo-trial
```

Then either run the app from the package:

```bash
swift run
```

or build an app bundle, which is also what lets it send notifications:

```bash
./build-app.sh
```

The engine's address and token are in the app's Settings. The default is `http://127.0.0.1:7878` with no token.

## Check it without a window

```bash
swift test
```

```bash
swift run FactoryApp --render out.png --api http://127.0.0.1:7878 --screen schedule
```

`--screen` is `issue` (the default), `schedule`, `workflow`, `usage` or `editor`. `--issue owner/repo#7` picks the issue, and `--draft <file>` gives the editor a draft to check.

The second draws the panes to an image from live data. Add `--editor` to draw the workflow editor's panes, and `--draft <file>` to draw them for a draft.

## Layout

| File | Holds |
| --- | --- |
| `Models.swift` | What the API returns, and how issues are grouped and labelled |
| `Client.swift` | The API client |
| `AppModel.swift` | What the windows show, refreshed every two seconds |
| `Views.swift` | The panes |
| `Graph.swift` | Where the workflow's steps and arrows go |
| `Editor.swift` | The workflow editor, the module list and the scaffold form |
| `App.swift` | The window, the menu bar item, settings |
| `Render.swift` | Drawing the panes to an image |
