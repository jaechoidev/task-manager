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

/// What the engine says about the draft, and its graph when it is valid.
struct ValidationPane: View {
    var validation: Validation?
    var changed: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let validation {
                if validation.ok {
                    Label(changed ? "Valid. Not on the default branch yet." : "Valid. This is the workflow on the default branch.", systemImage: "checkmark.circle.fill")
                        .foregroundStyle(.green).font(.callout)
                } else {
                    Label("\(validation.errors.count) problem\(validation.errors.count == 1 ? "" : "s"). A run would refuse this file.", systemImage: "xmark.octagon.fill")
                        .foregroundStyle(.red).font(.callout)
                    ForEach(Array(validation.errors.enumerated()), id: \.offset) { _, problem in
                        Text("• " + problem).font(.callout).textSelection(.enabled)
                    }
                }
                if let workflow = validation.workflow {
                    WorkflowPane(workflow: workflow).padding(.top, 6)
                }
            } else {
                Text("Checking...").font(.callout).foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// The modules a workflow in this repository may name.
struct ModuleListPane: View {
    var modules: [ModuleInfo]
    var insert: ((ModuleInfo) -> Void)?

    var body: some View {
        Card(title: "Modules") {
            ForEach(modules) { module in
                HStack(alignment: .top, spacing: 8) {
                    RoundedRectangle(cornerRadius: 3).fill(Color.forKind(module.kind).opacity(0.4)).frame(width: 10, height: 10).padding(.top, 4)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(module.module).font(.callout.monospaced())
                        Text([module.role ?? module.kind, module.source].joined(separator: ", ")).font(.caption).foregroundStyle(.secondary)
                        if let problem = module.problem {
                            Text(problem).font(.caption).foregroundStyle(.red)
                        } else {
                            Text("needs \(module.needs.joined(separator: ", ")). produces \(module.produces.joined(separator: ", "))")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    Spacer(minLength: 0)
                    if let insert {
                        Button("Add as a step") { insert(module) }.controlSize(.small)
                    }
                }
            }
        }
    }
}

struct WorkflowEditorPane: View {
    @ObservedObject var editor: EditorModel
    var client: APIClient

    var body: some View {
        HStack(alignment: .top, spacing: 16) {
            VStack(alignment: .leading, spacing: 8) {
                Text("factory.yaml of \(editor.repo)").font(.headline)
                TextEditor(text: $editor.draft)
                    .font(.callout.monospaced())
                    .frame(width: 520, height: 460)
                    .border(Color.gray.opacity(0.3))
                    .onChange(of: editor.draft) { _, _ in editor.draftChanged(client: client) }
                HStack {
                    Button("Open a pull request") { Task { await editor.propose(client: client) } }
                        .disabled(!editor.canPropose)
                        .keyboardShortcut("s")
                    Button("Revert") { editor.draft = editor.original }.disabled(!editor.changed)
                    if editor.busy { ProgressView().controlSize(.small) }
                }
                if let message = editor.message {
                    Text(message).font(.callout).textSelection(.enabled)
                        .padding(8).frame(maxWidth: 520, alignment: .leading)
                        .background(Color.accentColor.opacity(0.12), in: RoundedRectangle(cornerRadius: 6))
                }
                Card(title: "New module") {
                    HStack {
                        Picker("", selection: $editor.newKind) {
                            ForEach(["check", "agent", "judgment"], id: \.self) { Text($0) }
                        }
                        .labelsHidden().frame(width: 110)
                        TextField("name, such as lint", text: $editor.newName).frame(width: 160)
                        if editor.newKind == "agent" {
                            Picker("", selection: $editor.newRole) {
                                ForEach(["research", "acceptance", "dev", "review", "summary"], id: \.self) { Text($0) }
                            }
                            .labelsHidden().frame(width: 120)
                        }
                    }
                    Text(editor.newKind == "check"
                        ? "A check gets a module.yaml and a script that fails until someone writes it."
                        : "It starts as a copy of the built-in module, to edit in the pull request.")
                        .font(.caption).foregroundStyle(.secondary)
                    Button("Scaffold as a pull request") { Task { await editor.scaffold(client: client) } }
                        .disabled(editor.newName.trimmingCharacters(in: .whitespaces).isEmpty || editor.busy)
                }
                .frame(width: 520)
            }
            VStack(alignment: .leading, spacing: 14) {
                ValidationPane(validation: editor.validation, changed: editor.changed)
                ModuleListPane(modules: editor.modules) { module in
                    editor.draft = insertStep(into: editor.draft, module: module, developer: editor.developer)
                }
                .frame(width: 560)
            }
        }
    }
}
