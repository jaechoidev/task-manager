import AppKit
import SwiftUI

/// Draws the app's panes to a PNG from live data, with no window. For a
/// check of what the app shows: `FactoryApp --render out.png --api <address>`.
enum Render {
    static func fetch<T: Decodable>(_ address: String, _ path: String) -> T? {
        guard let url = URL(string: address + "/v1/" + path), let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder.api.decode(T.self, from: data)
    }

    @MainActor
    static func image(issues: [Issue], selected: Issue?, detail: IssueDetail?, log: LogTail?, workflow: Workflow?, usage: [UsageRow]) -> NSImage? {
        let view = HStack(alignment: .top, spacing: 0) {
            IssueListPane(issues: issues, selected: selected?.id)
                .frame(width: 300).padding(10)
            Divider()
            VStack(alignment: .leading, spacing: 18) {
                if let selected {
                    IssueDetailPane(issue: selected, detail: detail, log: log).frame(width: 640)
                }
                if let workflow {
                    WorkflowPane(workflow: workflow)
                }
                UsagePane(usage: usage).frame(width: 520)
            }
            .padding(16)
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .environment(\.colorScheme, .light)
        let renderer = ImageRenderer(content: view)
        renderer.scale = 2
        return renderer.nsImage
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

    /// Draws the workflow editor's panes for a draft: the text, what the
    /// engine says about it, and the modules. The draft is the default
    /// branch's file unless `draftFile` names another.
    static func editor(to path: String, address: String, draftFile: String?) -> Int32 {
        _ = NSApplication.shared
        guard let current: Workflow = fetch(address, "workflow") else {
            FileHandle.standardError.write(Data("can't read the workflow from \(address)\n".utf8))
            return 1
        }
        let original = current.text ?? ""
        let draft = draftFile.flatMap { try? String(contentsOfFile: $0, encoding: .utf8) } ?? original
        let modules: [ModuleInfo] = fetch(address, "modules") ?? []
        // The same check the editor makes on every change.
        var request = URLRequest(url: URL(string: address + "/v1/workflow/validate")!)
        request.httpMethod = "POST"
        request.httpBody = Data(draft.utf8)
        var validation: Validation?
        let done = DispatchSemaphore(value: 0)
        URLSession.shared.dataTask(with: request) { data, _, _ in
            validation = data.flatMap { try? JSONDecoder.api.decode(Validation.self, from: $0) }
            done.signal()
        }.resume()
        done.wait()
        let image = MainActor.assumeIsolated { () -> NSImage? in
            let view = HStack(alignment: .top, spacing: 16) {
                VStack(alignment: .leading, spacing: 8) {
                    Text("factory.yaml, the draft").font(.headline)
                    Text(draft).font(.caption.monospaced()).frame(width: 520, alignment: .leading)
                        .padding(8).border(Color.gray.opacity(0.3))
                }
                VStack(alignment: .leading, spacing: 14) {
                    ValidationPane(validation: validation, changed: draft != original)
                    ModuleListPane(modules: modules, insert: nil).frame(width: 560)
                }
            }
            .padding(16)
            .background(Color(nsColor: .windowBackgroundColor))
            .environment(\.colorScheme, .light)
            let renderer = ImageRenderer(content: view)
            renderer.scale = 2
            return renderer.nsImage
        }
        return write(image, to: path, note: "\(modules.count) module(s), draft \(validation?.ok == true ? "valid" : "not valid")")
    }

    static func run(to path: String, address: String, issue: String?) -> Int32 {
        _ = NSApplication.shared
        guard let issues: [Issue] = fetch(address, "issues") else {
            FileHandle.standardError.write(Data("can't reach the engine at \(address)\n".utf8))
            return 1
        }
        let selected = issues.first { $0.id == issue } ?? issues.first { $0.state == "needs-a-person" } ?? issues.first
        let detail: IssueDetail? = selected.flatMap { fetch(address, $0.path) }
        let log: LogTail? = selected.flatMap { fetch(address, $0.path + "/log") }
        let workflow: Workflow? = fetch(address, "workflow")
        let usage: [UsageRow] = fetch(address, "usage") ?? []
        let image = MainActor.assumeIsolated { Render.image(issues: issues, selected: selected, detail: detail, log: log, workflow: workflow, usage: usage) }
        guard let tiff = image?.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff), let png = bitmap.representation(using: .png, properties: [:]) else {
            FileHandle.standardError.write(Data("could not draw the image\n".utf8))
            return 1
        }
        do {
            try png.write(to: URL(fileURLWithPath: path))
            print("wrote \(path): \(issues.count) issue(s)")
            return 0
        } catch {
            FileHandle.standardError.write(Data("could not write \(path): \(error.localizedDescription)\n".utf8))
            return 1
        }
    }
}
