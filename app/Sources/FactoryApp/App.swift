import AppKit
import SwiftUI

/// Scrolls sideways in a window. Drawn to an image there is nothing to
/// scroll, and a scroll view draws nothing, so the content stands alone.
struct SidewaysScroll<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        if Render.drawing {
            content
        } else {
            ScrollView(.horizontal, showsIndicators: false) { content }
        }
    }
}

struct RootView: View {
    @ObservedObject var model: AppModel
    @StateObject private var editor = EditorModel()
    @State private var screen: Screen = .issue
    @State private var filter = ""
    @State private var sidebarShown = true

    var body: some View {
        HStack(spacing: 0) {
            if sidebarShown {
                ScrollView {
                    SidebarPane(
                        issues: model.issues.filter { matches($0, filter: filter, project: model.project) },
                        several: model.project == nil && Set(model.issues.map(\.repo)).count > 1,
                        parked: model.parked,
                        selected: model.selected,
                        screen: screen,
                        places: model.schedule?.schedulers.first.map { "\($0.inUse) of \($0.places)" },
                        filter: $filter,
                        go: { screen = $0 },
                        select: { id in
                            model.selected = id
                            screen = .issue
                            Task { await model.refresh() }
                        }
                    )
                }
                .frame(width: 300)
                .background(Theme.sidebar)
                Rectangle().fill(Color(hex: 0xDCDCDF)).frame(width: 1)
            }
            VStack(alignment: .leading, spacing: 0) {
                if let problem = model.problem {
                    Text(problem).font(.system(size: 13)).foregroundStyle(Theme.redText)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16).padding(.vertical, 10).background(Theme.redFill)
                }
                if let notice = model.notice {
                    Text(notice).font(.system(size: 13)).foregroundStyle(Theme.blueText)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 16).padding(.vertical, 8).background(Theme.blueCard)
                }
                content
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(Color.white)
        }
        .toolbar {
            ToolbarItemGroup(placement: .navigation) {
                Button { sidebarShown.toggle() } label: { Image(systemName: "sidebar.left") }
                    .help("Show or hide the sidebar")
                Menu(model.project ?? "All projects") {
                    Button("All projects") { model.project = nil }
                    Divider()
                    ForEach(model.projects, id: \.self) { repo in
                        Button(repo) { model.project = repo }
                    }
                }
                .help("Which project's issues, schedule and workflow are shown")
            }
            ToolbarItemGroup(placement: .primaryAction) {
                TimelineView(.periodic(from: .now, by: 1)) { moment in
                    let reached = model.problem == nil
                    HStack(spacing: 6) {
                        Circle().fill(reached ? Theme.green : Theme.red).frame(width: 7, height: 7)
                        Text(engineLine(address: model.address, reached: reached, updated: model.updatedAt, now: moment.date))
                            .font(.system(size: 13)).foregroundStyle(Theme.secondary)
                    }
                }
                Button { Task { await model.refresh() } } label: { Image(systemName: "arrow.clockwise") }
                    .help("Ask the engine again now")
            }
        }
        .preferredColorScheme(.light)
        .frame(minWidth: 1040, minHeight: 640)
        .task {
            Notifier.askOnce()
            model.start()
        }
    }

    @ViewBuilder
    private var content: some View {
        switch screen {
        case .editor:
            if let repo = model.workflowRepo {
                WorkflowEditorPane(editor: editor, client: model.client) { screen = .workflow }
                    .task(id: repo) { await editor.load(repo: repo, client: model.client) }
            } else {
                page { missing("Start the engine with a checkout of the repository to edit its workflow.") }
            }
        case .schedule:
            page {
                if let schedule = model.schedule {
                    SchedulePane(schedule: schedule, project: model.project) { id in
                        model.selected = id
                        screen = .issue
                        Task { await model.refresh() }
                    }
                } else {
                    missing("This engine does not report its schedule. Build a newer `factory`.")
                }
            }
        case .workflow:
            page {
                if let workflow = model.workflow {
                    WorkflowPane(workflow: workflow, repo: model.workflowRepo) { screen = .editor }
                } else {
                    missing("Start the engine with a checkout of the repository to see its workflow.")
                }
            }
        case .usage:
            page {
                UsagePane(usage: model.usage, schedule: model.schedule, refreshQuota: { Task { await model.refreshQuota() } }, refreshing: model.quotaRefreshing)
            }
        case .issue:
            page {
                if let issue = model.selectedIssue {
                    IssueDetailPane(
                        issue: issue, detail: model.detail, log: model.log,
                        order: model.workflow?.steps.map(\.name),
                        parked: model.parked.contains(issue.id),
                        looked: model.looked,
                        toggleLooked: { model.toggleLooked($0) },
                        retry: { Task { await model.act("retry", on: issue) } },
                        drop: { Task { await model.act("drop", on: issue) } }
                    )
                    .id(issue.id)
                } else {
                    missing("Pick an issue.")
                }
            }
        }
    }

    private func page<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        ScrollView {
            content()
                .padding(.horizontal, 36).padding(.top, 28).padding(.bottom, 40)
                .frame(maxWidth: .infinity, alignment: .topLeading)
        }
    }

    private func missing(_ text: String) -> some View {
        Text(text).font(.system(size: 13)).foregroundStyle(Theme.secondary)
    }
}

/// The line in the toolbar that says whether the engine answers.
func engineLine(address: String, reached: Bool, updated: Date?, now: Date) -> String {
    let host = address.replacingOccurrences(of: "http://", with: "").replacingOccurrences(of: "https://", with: "")
    guard reached, let updated else { return "No answer from the engine at \(host)" }
    return "Engine at \(host) · \(agoLine(max(0, Int(now.timeIntervalSince(updated)))))"
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
            let named = arguments.firstIndex(of: "--screen").flatMap { arguments.count > $0 + 1 ? Screen(rawValue: arguments[$0 + 1]) : nil }
            let screen = arguments.contains("--editor") ? Screen.editor : (named ?? .issue)
            exit(Render.run(to: arguments[index + 1], address: address, issue: issue, screen: screen, draftFile: draft))
        }
        FactoryApp.main()
    }
}
