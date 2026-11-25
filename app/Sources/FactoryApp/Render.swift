import AppKit
import SwiftUI

/// Draws one of the app's screens to a PNG from live data, with no window.
/// For a check of what the app shows:
/// `FactoryApp --render out.png --api <address> --screen schedule`.
enum Render {
    /// Set while drawing to an image. Views that need a window to work, such
    /// as a scroll view, draw their content plainly instead.
    nonisolated(unsafe) static var drawing = false

    static func fetch<T: Decodable>(_ address: String, _ path: String) -> T? {
        guard let url = URL(string: address + "/v1/" + path), let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder.api.decode(T.self, from: data)
    }

    /// What the engine says about a draft, asked the way the editor asks.
    static func validate(_ address: String, draft: String, repo: String?) -> Validation? {
        let query = repo.flatMap { $0.addingPercentEncoding(withAllowedCharacters: .urlQueryValueAllowed) }.map { "?repo=\($0)" } ?? ""
        guard let url = URL(string: address + "/v1/workflow/validate" + query) else { return nil }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.httpBody = Data(draft.utf8)
        var validation: Validation?
        let done = DispatchSemaphore(value: 0)
        URLSession.shared.dataTask(with: request) { data, _, _ in
            validation = data.flatMap { try? JSONDecoder.api.decode(Validation.self, from: $0) }
            done.signal()
        }.resume()
        done.wait()
        return validation
    }

    static func write(_ image: NSImage?, to path: String, note: String) -> Int32 {
        guard let tiff = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff), let png = bitmap.representation(using: .png, properties: [:]) else {
            FileHandle.standardError.write(Data("could not draw the image\n".utf8))
            return 1
        }
        do {
            try png.write(to: URL(fileURLWithPath: path))
            print("wrote \(path): \(note)")
            return 0
        } catch {
            FileHandle.standardError.write(Data("could not write \(path): \(error.localizedDescription)\n".utf8))
            return 1
        }
    }

    static func run(to path: String, address: String, issue: String?, screen: Screen, draftFile: String?) -> Int32 {
        _ = NSApplication.shared
        drawing = true
        guard let issues: [Issue] = fetch(address, "issues") else {
            FileHandle.standardError.write(Data("can't reach the engine at \(address)\n".utf8))
            return 1
        }
        let selected = issues.first { $0.id == issue } ?? issues.first { $0.state == "needs-a-person" } ?? issues.first
        let detail: IssueDetail? = selected.flatMap { fetch(address, $0.path) }
        let log: LogTail? = selected.flatMap { fetch(address, $0.path + "/log") }
        let repo = selected?.repo
        let query = repo.flatMap { $0.addingPercentEncoding(withAllowedCharacters: .urlQueryValueAllowed) }.map { "?repo=\($0)" } ?? ""
        let workflow: Workflow? = fetch(address, "workflow" + query)
        let usage: [UsageRow] = fetch(address, "usage") ?? []
        let schedule: Schedule? = fetch(address, "schedule")
        let modules: [ModuleInfo] = screen == .editor ? (fetch(address, "modules" + query) ?? []) : []
        let providers: ProvidersReport? = screen == .providers ? fetch(address, "providers") : nil
        let original = workflow?.text ?? ""
        let draft = draftFile.flatMap { try? String(contentsOfFile: $0, encoding: .utf8) } ?? original
        let validation = screen == .editor ? validate(address, draft: draft, repo: repo) : nil
        let parked = Set((schedule?.schedulers ?? []).flatMap { board in board.projects.flatMap { project in project.parked.map { "\(project.repo)#\($0.number)" } } })

        let image = MainActor.assumeIsolated { () -> NSImage? in
            let view = HStack(alignment: .top, spacing: 0) {
                SidebarPane(
                    issues: issues, several: Set(issues.map(\.repo)).count > 1, parked: parked, selected: selected?.id, screen: screen,
                    places: schedule?.schedulers.first.map { "\($0.inUse) of \($0.places)" }, filter: nil
                )
                .frame(width: 300)
                Rectangle().fill(Color(hex: 0xDCDCDF)).frame(width: 1)
                Group {
                    switch screen {
                    case .editor:
                        VStack(alignment: .leading, spacing: 0) {
                            EditorBar(repo: repo ?? "", changed: draft != original, problems: validation.map { $0.errors.count }, canPropose: draft != original && validation?.ok == true)
                            Rectangle().fill(Theme.border).frame(height: 1)
                            EditorNote()
                            DraftGraph(validation: validation, lastValid: workflow)
                            Rectangle().fill(Theme.border).frame(height: 1)
                            HStack(alignment: .top, spacing: 0) {
                                DraftLines(text: draft, errors: validation?.errors ?? [])
                                Rectangle().fill(Theme.border).frame(width: 1)
                                EditorInspector(text: draft, validation: validation, modules: modules, insert: { _ in }).frame(width: 380)
                            }
                        }
                        .frame(width: 1400)
                    case .schedule:
                        padded {
                            if let schedule { SchedulePane(schedule: schedule) } else { Text("This engine does not report its schedule.") }
                        }
                    case .workflow:
                        padded {
                            if let workflow { WorkflowPane(workflow: workflow, repo: repo, edit: {}) } else { Text("No workflow.") }
                        }
                    case .usage:
                        padded { UsagePane(usage: usage, schedule: schedule) }
                    case .providers:
                        padded {
                            if let providers { ProvidersPane(report: providers) } else { Text("This engine does not report its providers.") }
                        }
                    case .issue:
                        padded {
                            if let selected {
                                IssueDetailPane(issue: selected, detail: detail, log: log, order: workflow?.steps.map(\.name), parked: parked.contains(selected.id))
                            } else {
                                Text("No issue has been run yet.")
                            }
                        }
                    }
                }
                .background(Color.white)
            }
            .background(Theme.sidebar)
            .environment(\.colorScheme, .light)
            let renderer = ImageRenderer(content: view)
            renderer.scale = 2
            return renderer.nsImage
        }
        return write(image, to: path, note: "\(screen.rawValue) screen, \(issues.count) issue(s)")
    }

    @MainActor
    private static func padded<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        content()
            .padding(.horizontal, 36).padding(.top, 28).padding(.bottom, 40)
            .frame(width: 1420, alignment: .topLeading)
    }
}
