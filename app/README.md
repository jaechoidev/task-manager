# Factory, the macOS app

A window and a menu bar item onto the engine. It is a client of `factory serve` and of nothing else: every number it shows comes from the engine's API, and its two buttons call the same commands the CLI has.

## What it shows

| Part | What is there |
| --- | --- |
| Issue list | Every issue the engine has run, grouped: needs a person, running and waiting, pull request open, done |
| Issue | Its state and what it waits on, the summary written for a person, every step with who ran it and how long it took, and the end of the newest output |
| Needs you | For a stuck issue: what failed, what the judgment said, and two buttons, "Start over" and "Take off the queue" |
| Workflow | The repository's steps as a graph, with an arrow from each step to where its failure goes. Read only |
| Usage | Calls, time and usage limits hit, per vendor |
| Menu bar | How many issues need a person, and what is running |

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
swift run FactoryApp --render out.png --api http://127.0.0.1:7878
```

The second draws the panes to an image from live data.

## Layout

| File | Holds |
| --- | --- |
| `Models.swift` | What the API returns, and how issues are grouped and labelled |
| `Client.swift` | The API client |
| `AppModel.swift` | What the windows show, refreshed every two seconds |
| `Views.swift` | The panes |
| `Graph.swift` | Where the workflow's steps and arrows go |
| `App.swift` | The window, the menu bar item, settings |
| `Render.swift` | Drawing the panes to an image |
