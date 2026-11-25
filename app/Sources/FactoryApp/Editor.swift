import SwiftUI

/// Editing a repository's workflow. The file's text is what is edited: the
/// engine checks every draft exactly as a run would, and "saving" opens a
/// pull request, because a run only ever reads the default branch.
@MainActor
final class EditorModel: ObservableObject {
    @Published var repo = ""
    @Published var original = ""
    @Published var draft = ""
    @Published var validation: Validation?
    @Published var modules: [ModuleInfo] = []
    @Published var message: String?
    @Published var busy = false
    @Published var newKind = "check"
    @Published var newName = ""
    @Published var newRole = "review"
    @Published var moduleFilter = ""
    /// The last draft the engine accepted, as a graph. A draft it refuses
    /// has no graph of its own, so this one is shown with the refused steps
    /// marked.
    @Published var lastValid: Workflow?

    private var pending: Task<Void, Never>?

    var changed: Bool { draft != original }
    var canPropose: Bool { changed && (validation?.ok ?? false) && !busy }
    var developer: String? { validation?.workflow?.steps.first { $0.module == "agent/dev" || $0.name == "develop" }?.name }

    private func query(_ path: String) -> String {
        let encoded = repo.addingPercentEncoding(withAllowedCharacters: .urlQueryValueAllowed) ?? repo
        return "\(path)?repo=\(encoded)"
    }

    func load(repo: String, client: APIClient) async {
        guard repo != self.repo || original.isEmpty else { return }
        self.repo = repo
        do {
            let workflow: Workflow = try await client.get(query("workflow"))
            original = workflow.text ?? ""
            draft = original
            validation = Validation(ok: true, errors: [], workflow: workflow)
            lastValid = workflow
            modules = (try? await client.get(query("modules"))) ?? []
            message = nil
        } catch {
            message = "Could not read the workflow of \(repo): \(error.localizedDescription)"
        }
    }

    /// Checks the draft a moment after the last keystroke.
    func draftChanged(client: APIClient) {
        pending?.cancel()
        pending = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(500))
            guard !Task.isCancelled, let self else { return }
            await self.validate(client: client)
        }
    }

    func validate(client: APIClient) async {
        do {
            validation = try await client.send(query("workflow/validate"), body: Data(draft.utf8))
            if let workflow = validation?.workflow { lastValid = workflow }
        } catch {
            validation = Validation(ok: false, errors: ["The engine could not check this: \(error.localizedDescription)"], workflow: nil)
        }
    }

    func propose(client: APIClient) async {
        busy = true
        defer { busy = false }
        do {
            let result: Proposal = try await client.send(query("workflow/propose"), body: Data(draft.utf8))
            message = result.ok ? "Opened \(result.url ?? "a pull request"). Nothing changes until it is merged." : "Not proposed: \(result.problems.joined(separator: " "))"
        } catch {
            message = "Could not open the pull request: \(error.localizedDescription)"
        }
    }

    func scaffold(client: APIClient) async {
        busy = true
        defer { busy = false }
        var wanted: [String: String] = ["module": "\(newKind)/\(newName.trimmingCharacters(in: .whitespaces))"]
        if newKind == "agent" { wanted["role"] = newRole }
        do {
            let body = try JSONSerialization.data(withJSONObject: wanted)
            let result: Proposal = try await client.send(query("modules"), body: body, json: true)
            message = result.ok ? "Opened \(result.url ?? "a pull request") with \(result.files?.count ?? 0) file(s). The module can be used once it is merged." : "Not scaffolded: \(result.problems.joined(separator: " "))"
        } catch {
            message = "Could not scaffold the module: \(error.localizedDescription)"
        }
    }
}

/// Whether a workflow file's text names this module in a step. `check/verify`
/// is not named by a line that says `check/verifyy`.
func names(_ module: String, in text: String) -> Bool {
    var from = text.startIndex
    while let found = text.range(of: "module: \(module)", range: from..<text.endIndex) {
        let next = found.upperBound < text.endIndex ? text[found.upperBound] : " "
        if !(next.isLetter || next.isNumber || next == "-" || next == "_" || next == "/") { return true }
        from = found.upperBound
    }
    return false
}

/// A few words for a refused step's box, from the engine's sentence about it.
func refusalWords(_ problem: String) -> String {
    let lower = problem.lowercased()
    if lower.contains("does not exist") || lower.contains("not found") { return "module not found" }
    if let needs = problem.range(of: "needs '"), let end = problem[needs.upperBound...].firstIndex(of: "'") {
        return "needs \(problem[needs.upperBound..<end])"
    }
    if lower.contains("on_fail") { return "no way back" }
    return "refused"
}

/// The steps a draft's problems name, each with a few words on why.
func refusedSteps(_ errors: [String]) -> [String: String] {
    var found: [String: String] = [:]
    for problem in errors {
        if let step = problemStep(problem), found[step] == nil {
            found[step] = refusalWords(problem)
        }
    }
    return found
}

/// The workflow file as numbered lines, with the engine's word under each
/// line a problem is about. Read only: this is how a draft is drawn to an
/// image, where a text editor can't be.
struct DraftLines: View {
    var text: String
    var errors: [String]

    var body: some View {
        let byLine = Dictionary(errors.compactMap { problem in problemLine(problem, in: text).map { ($0, problem) } }, uniquingKeysWith: { first, _ in first })
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(text.components(separatedBy: "\n").enumerated()), id: \.offset) { index, line in
                let bad = byLine[index + 1]
                HStack(alignment: .top, spacing: 0) {
                    Text("\(index + 1)").foregroundStyle(bad == nil ? Theme.idle : Theme.redText).fontWeight(bad == nil ? .regular : .semibold)
                        .frame(width: 40, alignment: .trailing).padding(.trailing, 4)
                    Circle().fill(bad == nil ? Color.clear : Theme.red).frame(width: 8, height: 8).frame(width: 16, height: 20)
                    Text(colored(line))
                    Spacer(minLength: 0)
                }
                .frame(minHeight: 20)
                .background(bad == nil ? Color.clear : Color(hex: 0xFDF3F2))
                if let bad {
                    Text(bad).font(.system(size: 12)).foregroundStyle(Theme.redText)
                        .padding(.horizontal, 10).padding(.vertical, 5)
                        .background(Theme.redFill)
                        .overlay(alignment: .leading) { Rectangle().fill(Theme.red).frame(width: 3) }
                        .padding(.leading, 64).padding(.vertical, 2)
                }
            }
        }
        .font(.system(size: 12.5, design: .monospaced))
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.white)
    }

    /// Keys in blue and comments in gray, as far as a line's own text tells.
    private func colored(_ line: String) -> AttributedString {
        var code = line, comment = ""
        if let hash = line.range(of: "#"), hash.lowerBound == line.startIndex || line[line.index(before: hash.lowerBound)] == " " {
            code = String(line[..<hash.lowerBound])
            comment = String(line[hash.lowerBound...])
        }
        var out = AttributedString()
        if let colon = code.firstIndex(of: ":"), code[..<colon].allSatisfy({ $0.isLetter || $0 == "_" || $0 == " " || $0 == "-" }) {
            var key = AttributedString(String(code[..<colon]))
            key.foregroundColor = Color(hex: 0x1F5FA8)
            var rest = AttributedString(String(code[colon...]))
            rest.foregroundColor = Theme.text
            out = key + rest
        } else {
            out = AttributedString(code.isEmpty && comment.isEmpty ? " " : code)
            out.foregroundColor = Theme.text
        }
        var tail = AttributedString(comment)
        tail.foregroundColor = Theme.secondary
        return out + tail
    }
}

/// Beside the draft: what the engine says about it, the modules it may
/// name, and a form that scaffolds a new one.
struct EditorInspector: View {
    var text: String
    var validation: Validation?
    var modules: [ModuleInfo]
    /// The form's fields. Nil draws them without controls, for an image.
    var editor: EditorModel?
    var insert: ((ModuleInfo) -> Void)?
    var scaffold: (() -> Void)?

    var body: some View {
        let errors = validation?.errors ?? []
        let filter = (editor?.moduleFilter ?? "").lowercased()
        let shown = modules.filter { filter.isEmpty || $0.module.lowercased().contains(filter) }
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("Problems").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                    Spacer()
                    Text("\(errors.count)").font(.system(size: 13, weight: .semibold)).foregroundStyle(errors.isEmpty ? Theme.secondary : Theme.redText)
                }
                Text(validation == nil ? "Checking..." : (errors.isEmpty ? "A run would accept this file." : "A run would refuse this file."))
                    .font(.system(size: 12)).foregroundStyle(Theme.secondary)
                ForEach(Array(errors.enumerated()), id: \.offset) { _, problem in
                    HStack(alignment: .top, spacing: 10) {
                        if let line = problemLine(problem, in: text) {
                            Text("L\(line)").font(.system(size: 12, design: .monospaced)).foregroundStyle(Theme.secondary)
                        }
                        Text(problem).font(.system(size: 13)).foregroundStyle(Theme.text).textSelection(.enabled)
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 10).padding(.vertical, 8)
                    .background(Color.white, in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.redLine, lineWidth: 1))
                }
            }
            .padding(.horizontal, 18).padding(.vertical, 16)
            Rectangle().fill(Theme.border).frame(height: 1)

            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("Modules").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                    Spacer()
                    Text("\(modules.count)").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                }
                HStack(spacing: 6) {
                    Image(systemName: "magnifyingglass").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                    if let editor {
                        TextField("Filter modules", text: Binding(get: { editor.moduleFilter }, set: { editor.moduleFilter = $0 })).textFieldStyle(.plain).font(.system(size: 13))
                    } else {
                        Text("Filter modules").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                        Spacer(minLength: 0)
                    }
                }
                .padding(.horizontal, 8).frame(height: 28)
                .background(Color(hex: 0xE8E8EB), in: RoundedRectangle(cornerRadius: 6))
                ForEach(shown) { module in
                    moduleCard(module)
                }
            }
            .padding(.horizontal, 18).padding(.vertical, 16)
            Rectangle().fill(Theme.border).frame(height: 1)

            newModule
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color(hex: 0xF7F7F8))
    }

    private func moduleCard(_ module: ModuleInfo) -> some View {
        let used = names(module.module, in: text)
        return HStack(alignment: .top, spacing: 10) {
            RoundedRectangle(cornerRadius: 3).fill(Theme.node(module.kind).line).frame(width: 10, height: 10).padding(.top, 4)
            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 6) {
                    Text(module.module).font(.system(size: 12.5, weight: .medium, design: .monospaced)).foregroundStyle(Theme.text)
                    if used {
                        Text("in use").font(.system(size: 11)).foregroundStyle(Theme.greenText)
                            .padding(.horizontal, 5).background(Theme.greenFill, in: RoundedRectangle(cornerRadius: 4))
                    }
                }
                if let problem = module.problem {
                    Text(problem).font(.system(size: 12)).foregroundStyle(Theme.redText)
                } else {
                    Text("needs \(module.needs.joined(separator: ", "))").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                    Text("produces \(module.produces.joined(separator: ", "))").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                }
                if module.source != "built-in" {
                    Text(module.source).font(.system(size: 12)).foregroundStyle(Theme.secondary)
                }
            }
            Spacer(minLength: 0)
            if !used, let insert {
                Text("Add").font(.system(size: 12)).foregroundStyle(Theme.text)
                    .padding(.horizontal, 8).frame(height: 24)
                    .background(Color.white, in: RoundedRectangle(cornerRadius: 6))
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.control, lineWidth: 1))
                    .contentShape(Rectangle())
                    .onTapGesture { insert(module) }
                    .accessibilityLabel("Add \(module.module) as a step")
                    .accessibilityAddTraits(.isButton)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 8)
        .background(Color.white, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.border, lineWidth: 1))
    }

    private var newModule: some View {
        let kind = editor?.newKind ?? "check"
        return VStack(alignment: .leading, spacing: 10) {
            Text("New module").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
            HStack(spacing: 0) {
                ForEach(["check", "agent", "judgment"], id: \.self) { option in
                    Text(option).font(.system(size: 13)).foregroundStyle(option == kind ? Theme.text : Theme.tertiary)
                        .frame(maxWidth: .infinity).frame(height: 24)
                        .background(option == kind ? Color.white : Color.clear, in: RoundedRectangle(cornerRadius: 5))
                        .shadow(color: option == kind ? .black.opacity(0.12) : .clear, radius: 1, y: 1)
                        .contentShape(Rectangle())
                        .onTapGesture { editor?.newKind = option }
                }
            }
            .padding(2)
            .background(Color(hex: 0xE8E8EB), in: RoundedRectangle(cornerRadius: 7))
            VStack(alignment: .leading, spacing: 4) {
                Text("Name").font(.system(size: 12)).foregroundStyle(Theme.tertiary)
                HStack(spacing: 0) {
                    Text("\(kind)/").foregroundStyle(Theme.secondary)
                    if let editor {
                        TextField("lint", text: Binding(get: { editor.newName }, set: { editor.newName = $0 })).textFieldStyle(.plain)
                    } else {
                        Text("lint").foregroundStyle(Theme.idle)
                        Spacer(minLength: 0)
                    }
                }
                .font(.system(size: 12.5, design: .monospaced))
                .padding(.horizontal, 8).frame(height: 28)
                .background(Color.white, in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.control, lineWidth: 1))
            }
            if kind == "agent", let editor {
                Picker("Role", selection: Binding(get: { editor.newRole }, set: { editor.newRole = $0 })) {
                    ForEach(["research", "acceptance", "dev", "review", "summary"], id: \.self) { Text($0) }
                }
                .font(.system(size: 12))
            }
            Text(kind == "check"
                ? "A check gets a module.yaml and a script that fails until someone writes it."
                : "It starts as a copy of the built-in module, to edit in the pull request.")
                .font(.system(size: 12)).foregroundStyle(Theme.secondary)
            OutlineButton(title: "Scaffold as a pull request", enabled: !(editor?.newName.trimmingCharacters(in: .whitespaces).isEmpty ?? true) && !(editor?.busy ?? false)) { scaffold?() }
        }
        .padding(.horizontal, 18).padding(.vertical, 16)
    }
}

/// The strip above the draft: its graph as the engine reads it. A refused
/// draft has no graph, so the last accepted one is drawn with the refused
/// steps marked.
struct DraftGraph: View {
    var validation: Validation?
    var lastValid: Workflow?

    var body: some View {
        let errors = validation?.errors ?? []
        VStack(alignment: .leading, spacing: 10) {
            Flow(spacing: 16) {
                Text("Graph of the draft").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                Text(errors.isEmpty ? "redrawn after each pause in typing" : "the last draft the engine accepted, with what it now refuses")
                    .font(.system(size: 13)).foregroundStyle(Theme.secondary)
                GraphLegend(withRefused: true)
            }
            if let workflow = validation?.workflow ?? lastValid {
                SidewaysScroll {
                    WorkflowGraph(workflow: workflow, refused: refusedSteps(errors)).padding(.vertical, 4)
                }
            } else {
                Text("No graph yet.").font(.system(size: 13)).foregroundStyle(Theme.secondary)
            }
        }
        .padding(.horizontal, 36).padding(.top, 12).padding(.bottom, 14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(hex: 0xFAFAFB))
    }
}

/// The bar over the editor: where you are, how the draft stands, and the
/// two things to do with it.
struct EditorBar: View {
    var repo: String
    var changed: Bool
    var problems: Int?
    var canPropose: Bool
    var busy = false
    var back: () -> Void = {}
    var revert: () -> Void = {}
    var propose: () -> Void = {}

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "chevron.left").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.tertiary)
                .frame(width: 32, height: 28).contentShape(Rectangle())
                .onTapGesture(perform: back)
                .accessibilityLabel("Back to workflow").accessibilityAddTraits(.isButton)
            HStack(spacing: 6) {
                Text(repo).foregroundStyle(Theme.secondary)
                Image(systemName: "chevron.right").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.idle)
                Text("factory.yaml").fontWeight(.semibold).foregroundStyle(Theme.text)
            }
            .font(.system(size: 13))
            Text(changed ? "Draft · edited" : "As on the default branch").font(.system(size: 12)).foregroundStyle(Theme.secondary)
            Spacer(minLength: 0)
            if let problems {
                if problems == 0 {
                    Text("✓ No problems").font(.system(size: 13)).foregroundStyle(Theme.greenText)
                } else {
                    Text("✕ \(problems) problem\(problems == 1 ? "" : "s")").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.redText)
                }
            }
            if busy { ProgressView().controlSize(.small) }
            OutlineButton(title: "Revert", enabled: changed, action: revert)
            HStack(spacing: 8) {
                Text("Open a pull request")
                Text("⌘S").opacity(0.7)
            }
            .font(.system(size: 13, weight: .medium)).foregroundStyle(canPropose ? Color.white : Theme.idle)
            .padding(.horizontal, 12).frame(height: 28)
            .background(canPropose ? Theme.accent : Theme.hairline, in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
            .onTapGesture { if canPropose { propose() } }
            .help(canPropose ? "Opens a pull request with this draft" : (changed ? "Fix the problems first" : "Nothing has changed"))
            .accessibilityAddTraits(.isButton)
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .background(Theme.band)
    }
}

/// The note under the bar: why saving is a pull request.
struct EditorNote: View {
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "info.circle").font(.system(size: 13))
            Text("Runs only read the default branch, so saving opens a pull request. The engine checks the draft as you type.").font(.system(size: 13))
            Spacer(minLength: 0)
        }
        .foregroundStyle(Theme.blueText)
        .padding(.horizontal, 16).padding(.vertical, 8)
        .background(Theme.blueCard)
    }
}

struct WorkflowEditorPane: View {
    @ObservedObject var editor: EditorModel
    var client: APIClient
    var back: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            EditorBar(
                repo: editor.repo, changed: editor.changed, problems: editor.validation.map { $0.errors.count }, canPropose: editor.canPropose, busy: editor.busy,
                back: back,
                revert: { editor.draft = editor.original },
                propose: { Task { await editor.propose(client: client) } }
            )
            // The shortcut needs a real button. It is not drawn.
            .background(Button("") { if editor.canPropose { Task { await editor.propose(client: client) } } }.keyboardShortcut("s").opacity(0))
            Rectangle().fill(Theme.border).frame(height: 1)
            EditorNote()
            if let message = editor.message {
                Text(message).font(.system(size: 13)).foregroundStyle(Theme.text).textSelection(.enabled)
                    .padding(.horizontal, 16).padding(.vertical, 8).frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.amberCard)
            }
            DraftGraph(validation: editor.validation, lastValid: editor.lastValid)
            Rectangle().fill(Theme.border).frame(height: 1)
            HStack(alignment: .top, spacing: 0) {
                TextEditor(text: $editor.draft)
                    .font(.system(size: 12.5, design: .monospaced))
                    .scrollContentBackground(.hidden)
                    .padding(.horizontal, 12).padding(.vertical, 10)
                    .background(Color.white)
                    .frame(minWidth: 420, maxWidth: .infinity, minHeight: 520, maxHeight: .infinity)
                    .onChange(of: editor.draft) { _, _ in editor.draftChanged(client: client) }
                Rectangle().fill(Theme.border).frame(width: 1)
                ScrollView {
                    EditorInspector(
                        text: editor.draft, validation: editor.validation, modules: editor.modules, editor: editor,
                        insert: { module in editor.draft = insertStep(into: editor.draft, module: module, developer: editor.developer) },
                        scaffold: { Task { await editor.scaffold(client: client) } }
                    )
                }
                .frame(width: 380)
                .background(Color(hex: 0xF7F7F8))
            }
        }
    }
}
