import AppKit
import SwiftUI

struct RootView: View {
    @ObservedObject var model: AppModel
    @StateObject private var editor = EditorModel()
    @State private var tab = "Issue"

    var body: some View {
        NavigationSplitView {
            ScrollView {
                IssueListPane(issues: model.issues, selected: model.selected) { model.selected = $0; Task { await model.refresh() } }
                    .padding(8)
            }
            .navigationSplitViewColumnWidth(min: 260, ideal: 300)
        } detail: {
            VStack(alignment: .leading, spacing: 0) {
                if let problem = model.problem {
                    Text(problem).font(.callout).foregroundStyle(.white)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(10).background(.red.opacity(0.85))
                }
                if let notice = model.notice {
                    Text(notice).font(.callout).frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8).background(Color.accentColor.opacity(0.15))
                }
                Picker("", selection: $tab) {
                    ForEach(["Issue", "Schedule", "Workflow", "Edit workflow", "Usage"], id: \.self) { Text($0) }
                }
                .pickerStyle(.segmented).labelsHidden().padding(12).frame(maxWidth: 620)
                ScrollView([.vertical, .horizontal]) {
                    Group {
                        switch tab {
                        case "Workflow":
                            if let workflow = model.workflow {
                                WorkflowPane(workflow: workflow)
                            } else {
                                Text("Start the engine with `--repo-dir <checkout>` to see a repository's workflow.").foregroundStyle(.secondary)
                            }
                        case "Edit workflow":
                            if let repo = model.selectedIssue?.repo ?? model.repos.first, model.repos.contains(repo) {
                                WorkflowEditorPane(editor: editor, client: model.client)
                                    .task(id: repo) { await editor.load(repo: repo, client: model.client) }
                            } else {
                                Text("Start the engine with `--repo-dir <checkout>` to edit a repository's workflow.").foregroundStyle(.secondary)
                            }
                        case "Schedule":
                            if let schedule = model.schedule {
                                SchedulePane(schedule: schedule) { id in
                                    model.selected = id
                                    tab = "Issue"
                                    Task { await model.refresh() }
                                }
                                .frame(width: 640)
                            } else {
                                Text("This engine does not report its schedule. Build a newer `factory`.").foregroundStyle(.secondary)
                            }
                        case "Usage":
                            UsagePane(usage: model.usage).frame(width: 520)
                        default:
                            if let issue = model.selectedIssue {
                                IssueDetailPane(
                                    issue: issue, detail: model.detail, log: model.log,
                                    retry: { Task { await model.act("retry", on: issue) } },
                                    drop: { Task { await model.act("drop", on: issue) } }
                                )
                                .frame(width: 640)
                            } else {
                                Text("Pick an issue.").foregroundStyle(.secondary)
                            }
                        }
                    }
                    .padding(16)
                }
            }
        }
        .frame(minWidth: 980, minHeight: 620)
        .task {
            Notifier.askOnce()
            model.start()
        }
    }
}

/// What the menu bar shows: how many issues need a person, and what runs.
struct MenuContent: View {
    @ObservedObject var model: AppModel

    var body: some View {
        if let problem = model.problem {
            Text(problem)
        }
        ForEach(model.schedule?.schedulers ?? []) { board in
            Text("Scheduler: \(placesLine(board))")
        }
        if model.needingAPerson.isEmpty && model.active.isEmpty {
            Text("Nothing is running, and nothing needs you")
        }
        ForEach(model.needingAPerson) { issue in
            Text("Needs you: \(shortRepo(issue.repo)) #\(issue.number) \(issue.title)")
        }
        ForEach(model.active) { issue in
            Text("\(stateLabel(issue)): \(shortRepo(issue.repo)) #\(issue.number) \(issue.title)")
        }
        Divider()
        Button("Quit Factory") { NSApplication.shared.terminate(nil) }
    }
}

struct FactoryApp: App {
    @StateObject private var model = AppModel()

    init() {
        // A build run straight from `swift run` has no bundle. This makes it
        // a normal app with a window and a Dock icon anyway.
        NSApplication.shared.setActivationPolicy(.regular)
        NSApplication.shared.activate(ignoringOtherApps: true)
    }

    var body: some Scene {
        WindowGroup("Factory") {
            RootView(model: model)
        }
        MenuBarExtra {
            MenuContent(model: model)
        } label: {
            let stuck = model.needingAPerson.count
            Label(stuck > 0 ? "\(stuck)" : "", systemImage: stuck > 0 ? "exclamationmark.triangle.fill" : "hammer")
        }
        Settings {
            Form {
                TextField("Engine address", text: $model.address)
                SecureField("Token, if the engine needs one", text: $model.token)
            }
            .padding(20).frame(width: 420)
        }
    }
}

@main
enum Entry {
    static func main() {
        let arguments = CommandLine.arguments
        if let index = arguments.firstIndex(of: "--render"), arguments.count > index + 1 {
            let address = arguments.firstIndex(of: "--api").flatMap { arguments.count > $0 + 1 ? arguments[$0 + 1] : nil } ?? "http://127.0.0.1:7878"
            let issue = arguments.firstIndex(of: "--issue").flatMap { arguments.count > $0 + 1 ? arguments[$0 + 1] : nil }
            let draft = arguments.firstIndex(of: "--draft").flatMap { arguments.count > $0 + 1 ? arguments[$0 + 1] : nil }
            if arguments.contains("--editor") {
                exit(Render.editor(to: arguments[index + 1], address: address, draftFile: draft))
            }
            exit(Render.run(to: arguments[index + 1], address: address, issue: issue))
        }
        FactoryApp.main()
    }
}
