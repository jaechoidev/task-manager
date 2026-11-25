import SwiftUI

// The Providers screen: which model each tier names, and who is tried for
// each job. It edits this machine's providers.yaml through the engine, which
// checks every change and keeps the file's comments. What the engine may use
// is not decided here: a tier whose model changes has to pass its check
// again first.

/// What `GET /v1/providers` answers.
struct ProvidersReport: Codable, Equatable {
    var file: String
    var found: Bool
    var providers: [ProviderInfo]
    var roles: [String: [RoleEntry]]
    var problems: [String]
    /// The jobs a list can be given for, in the order a run does them.
    var roleNames: [String]?
    /// Tiers being checked right now, as `provider/tier`.
    var checking: [String]?
}

struct ProviderInfo: Codable, Equatable, Identifiable {
    var name: String
    var cli: String
    var program: String?
    var login: LoginInfo
    var quota: String?
    var tiers: [TierInfo]
    var id: String { name }
}

struct LoginInfo: Codable, Equatable {
    /// `default`, `env_file` or `config_dir`.
    var kind: String
    var path: String?
    var exists: Bool?
}

struct TierInfo: Codable, Equatable, Identifiable {
    var tier: String
    var model: String
    var family: String?
    /// What the check found for this model. Nil when it was never tried with it.
    var check: TierCheck?
    var outUntil: Int?
    var id: String { tier }
}

struct TierCheck: Codable, Equatable {
    var qualified: Bool
    var structured: Bool?
    var wroteInside: Bool?
    var wroteOutside: Bool?
    var wroteInHome: Bool?
    var readOutside: Bool?
    var error: String?
}

struct RoleEntry: Codable, Equatable {
    var provider: String
    var tier: String
    var model: String?
    var family: String?
    /// Whether the engine may use it: it passed its check with this model.
    var usable: Bool
}

/// One entry of a job's list, as it is sent to the engine.
struct RoleChoice: Codable, Equatable, Hashable {
    var use: String
    var tier: String
}

struct ProvidersAnswer: Codable {
    var ok: Bool
    var problems: [String]
    var changedTiers: [String]?
    var checking: [String]?
    var providers: ProvidersReport?
}

/// What a tier's line says about whether the engine may use it.
/// `kind` is `ok`, `warn`, `bad`, `busy` or `draft`.
func tierState(_ tier: TierInfo, checking: Bool, draft: String?) -> (text: String, kind: String) {
    if let draft, draft != tier.model {
        return ("changed, not saved. It has to pass its check again", "draft")
    }
    if checking {
        return ("being checked now: three short calls", "busy")
    }
    guard let check = tier.check else {
        return ("not checked with this model. The engine does not use it", "warn")
    }
    if check.qualified {
        return (check.readOutside == true ? "qualified. It can read outside its folder" : "qualified", "ok")
    }
    if let error = check.error {
        return ("failed its check: \(error)", "bad")
    }
    var why: [String] = []
    if check.structured == false { why.append("no structured reply") }
    if check.wroteInside == false { why.append("could not write in its folder") }
    if check.wroteOutside == true { why.append("wrote next to its folder") }
    if check.wroteInHome == true { why.append("wrote into the home directory") }
    return ("failed its check: " + (why.isEmpty ? "see its log" : why.joined(separator: ", ")), "bad")
}

/// A job's list with one entry moved up or down. Out of range changes nothing.
func moved(_ list: [RoleChoice], at index: Int, by step: Int) -> [RoleChoice] {
    let target = index + step
    guard list.indices.contains(index), list.indices.contains(target) else { return list }
    var out = list
    out.swapAt(index, target)
    return out
}

/// The body of a save: only what was changed. `roles`, when lists were
/// edited, is every list there is to be.
func providersChange(models: [String: String], added: [String: String], roles: [String: [RoleChoice]]?, check: Bool) -> [String: Any] {
    var tiers: [[String: String]] = []
    for (key, model) in models.merging(added, uniquingKeysWith: { _, new in new }).sorted(by: { $0.key < $1.key }) {
        let parts = key.split(separator: "/", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { continue }
        tiers.append(["provider": parts[0], "tier": parts[1], "model": model])
    }
    var body: [String: Any] = ["tiers": tiers, "check": check]
    if let roles {
        body["roles"] = roles.mapValues { $0.map { ["use": $0.use, "tier": $0.tier] } }
    }
    return body
}

@MainActor
final class ProvidersModel: ObservableObject {
    @Published var report: ProvidersReport?
    /// Models typed over the saved ones, by `provider/tier`.
    @Published var models: [String: String] = [:]
    /// Tiers to add, by `provider/tier`.
    @Published var added: [String: String] = [:]
    /// The lists as edited. Nil while nobody has touched them.
    @Published var roles: [String: [RoleChoice]]?
    @Published var problems: [String] = []
    @Published var message: String?
    @Published var busy = false
    /// The job an entry is being added to.
    @Published var adding: String?
    @Published var newTier: [String: String] = [:]
    @Published var newModel: [String: String] = [:]

    var changed: Bool {
        guard let report else { return false }
        let edited = models.contains { key, model in
            let parts = key.split(separator: "/", maxSplits: 1).map(String.init)
            return parts.count == 2 && report.providers.first { $0.name == parts[0] }?.tiers.first { $0.tier == parts[1] }?.model != model
        }
        return edited || !added.isEmpty || (roles != nil && roles != saved(report))
    }

    /// The lists as the file has them.
    func saved(_ report: ProvidersReport) -> [String: [RoleChoice]] {
        report.roles.mapValues { $0.map { RoleChoice(use: $0.provider, tier: $0.tier) } }
    }

    func list(_ role: String) -> [RoleChoice] {
        (roles ?? report.map(saved) ?? [:])[role] ?? []
    }

    func setList(_ role: String, _ list: [RoleChoice]) {
        var all = roles ?? report.map(saved) ?? [:]
        all[role] = list.isEmpty ? nil : list
        roles = all
    }

    func load(client: APIClient) async {
        if let fresh: ProvidersReport = try? await client.get("providers") {
            report = fresh
        }
    }

    func revert() {
        models = [:]
        added = [:]
        roles = nil
        problems = []
        message = nil
        adding = nil
    }

    func addTier(_ provider: String) {
        let name = (newTier[provider] ?? "").trimmingCharacters(in: .whitespaces)
        let model = (newModel[provider] ?? "").trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty, !model.isEmpty else { return }
        added["\(provider)/\(name)"] = model
        newTier[provider] = ""
        newModel[provider] = ""
    }

    func save(client: APIClient) async {
        busy = true
        defer { busy = false }
        do {
            let body = try JSONSerialization.data(withJSONObject: providersChange(models: models, added: added, roles: roles, check: true))
            let answer: ProvidersAnswer = try await client.send("providers", body: body, json: true)
            if answer.ok {
                if let fresh = answer.providers { report = fresh }
                let checking = answer.checking ?? []
                revert()
                message = checking.isEmpty ? "Saved." : "Saved. Checking \(checking.joined(separator: ", ")) now. The engine uses a tier once it has passed."
            } else {
                problems = answer.problems
            }
        } catch {
            problems = ["Could not save: \(error.localizedDescription)"]
        }
    }

    func check(_ provider: String, _ tier: String, client: APIClient) async {
        do {
            let body = try JSONSerialization.data(withJSONObject: ["provider": provider, "tier": tier])
            let answer: ProvidersAnswer = try await client.send("providers/check", body: body, json: true)
            message = answer.ok ? "Checking \(provider)/\(tier) now: three short calls." : answer.problems.joined(separator: " ")
        } catch {
            message = "Could not start the check: \(error.localizedDescription)"
        }
        await load(client: client)
    }
}

struct ProvidersPane: View {
    var report: ProvidersReport
    /// The edits and the actions. Nil draws the screen as saved, for an image.
    var model: ProvidersModel?
    var save: () -> Void = {}
    var check: (String, String) -> Void = { _, _ in }

    var body: some View {
        let changed = model?.changed ?? false
        VStack(alignment: .leading, spacing: 28) {
            HStack(alignment: .bottom, spacing: 12) {
                ScreenTitle(over: report.file, title: "Providers", under: "Which model each tier names, and who is tried for each job, in order.")
                Spacer(minLength: 0)
                if model?.busy == true { ProgressView().controlSize(.small) }
                OutlineButton(title: "Revert", enabled: changed) { model?.revert() }
                Button(action: save) {
                    Text("Save and check").font(.system(size: 13, weight: .medium)).foregroundStyle(changed ? Color.white : Theme.idle)
                        .padding(.horizontal, 12).frame(height: 28)
                        .background(changed ? Theme.accent : Theme.hairline, in: RoundedRectangle(cornerRadius: 6))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain).disabled(!changed)
                .help("Writes the file. A tier whose model changed is checked at once: three short calls")
            }

            let notes = report.problems + (model?.problems ?? [])
            if !notes.isEmpty {
                Boxed(fill: Theme.redFill, line: Theme.redLine) {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(Array(notes.enumerated()), id: \.offset) { _, note in
                            Text(note).font(.system(size: 13)).foregroundStyle(Theme.redText).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                    .padding(.horizontal, 16).padding(.vertical, 12)
                }
            }
            if let message = model?.message {
                Text(message).font(.system(size: 13)).foregroundStyle(Theme.blueText)
                    .padding(.horizontal, 16).padding(.vertical, 8).frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.blueCard, in: RoundedRectangle(cornerRadius: 8))
            }

            VStack(alignment: .leading, spacing: 12) {
                SectionTitle(title: "Jobs", note: "the first in a list that passed its check, is independent and has quota answers")
                Boxed {
                    VStack(spacing: 0) {
                        ForEach(Array((report.roleNames ?? report.roles.keys.sorted()).enumerated()), id: \.offset) { index, role in
                            if index > 0 { Rectangle().fill(Theme.hairline).frame(height: 1) }
                            jobRow(role)
                        }
                    }
                }
                Text("The reviewer and the acceptance writer never come from the developer's family. That is fixed in the engine and is not a setting.")
                    .font(.system(size: 12)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            }

            VStack(alignment: .leading, spacing: 12) {
                SectionTitle(title: "Providers", note: "a tier is a name for a model. A changed model has to pass its check before the engine uses it")
                ForEach(report.providers) { provider in
                    providerCard(provider)
                }
            }
        }
        .frame(maxWidth: 1080, alignment: .leading)
    }

    private func entries(_ role: String) -> [RoleChoice] {
        model?.list(role) ?? (report.roles[role] ?? []).map { RoleChoice(use: $0.provider, tier: $0.tier) }
    }

    private func tierInfo(_ choice: RoleChoice) -> TierInfo? {
        report.providers.first { $0.name == choice.use }?.tiers.first { $0.tier == choice.tier }
    }

    private func jobRow(_ role: String) -> some View {
        let list = entries(role)
        let offered = report.providers.flatMap { provider in provider.tiers.map { RoleChoice(use: provider.name, tier: $0.tier) } }.filter { !list.contains($0) }
        return HStack(alignment: .top, spacing: 12) {
            Text(role).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text).frame(width: 96, alignment: .leading).padding(.top, 4)
            VStack(alignment: .leading, spacing: 6) {
                if list.isEmpty {
                    Text("No list. The workflow's own vendor does this job, with its default model on this machine.").font(.system(size: 13)).foregroundStyle(Theme.secondary).padding(.top, 4)
                }
                ForEach(Array(list.enumerated()), id: \.offset) { index, choice in
                    let info = tierInfo(choice)
                    let usable = info?.check?.qualified == true
                    HStack(spacing: 8) {
                        Text("\(index + 1).").font(.system(size: 12).monospacedDigit()).foregroundStyle(Theme.secondary).frame(width: 18, alignment: .trailing)
                        Text("\(choice.use) \(choice.tier)").font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.text)
                        Text(info?.model ?? "?").font(.system(size: 12, design: .monospaced)).foregroundStyle(Theme.vendor(choice.use).text)
                        if !usable {
                            Text("not usable until it passes its check").font(.system(size: 12)).foregroundStyle(Theme.amber)
                        }
                        Spacer(minLength: 0)
                        if let model {
                            small("↑", "Move \(choice.use) \(choice.tier) up in \(role)", enabled: index > 0) { model.setList(role, moved(list, at: index, by: -1)) }
                            small("↓", "Move \(choice.use) \(choice.tier) down in \(role)", enabled: index < list.count - 1) { model.setList(role, moved(list, at: index, by: 1)) }
                            small("✕", "Take \(choice.use) \(choice.tier) out of \(role)") { model.setList(role, list.filter { $0 != choice }) }
                        }
                    }
                    .padding(.horizontal, 8).frame(height: 28)
                    .background(Theme.band, in: RoundedRectangle(cornerRadius: 6))
                }
                if let model, !offered.isEmpty {
                    if model.adding == role {
                        Flow(spacing: 6, lineSpacing: 6) {
                            ForEach(offered, id: \.self) { choice in
                                small("+ \(choice.use) \(choice.tier)", "Add \(choice.use) \(choice.tier) to \(role)") {
                                    model.setList(role, list + [choice])
                                }
                            }
                            small("Done", "Stop adding to \(role)") { model.adding = nil }
                        }
                    } else {
                        small("Add…", "Add someone to \(role)") { model.adding = role }
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
    }

    private func small(_ title: String, _ label: String, enabled: Bool = true, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).font(.system(size: 12)).foregroundStyle(enabled ? Theme.text : Theme.idle)
                .padding(.horizontal, 8).frame(minWidth: 24, minHeight: 22)
                .background(Color.white, in: RoundedRectangle(cornerRadius: 5))
                .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(Theme.control, lineWidth: 1))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain).disabled(!enabled).accessibilityLabel(label)
    }

    private func providerCard(_ provider: ProviderInfo) -> some View {
        let login: String = {
            switch provider.login.kind {
            case "env_file": return "env file \(provider.login.path ?? "")" + (provider.login.exists == false ? " (missing)" : "")
            case "config_dir": return "config folder \(provider.login.path ?? "")" + (provider.login.exists == false ? " (missing)" : "")
            default: return "the program's own login"
            }
        }()
        let added = (model?.added ?? [:]).filter { $0.key.hasPrefix(provider.name + "/") }.sorted { $0.key < $1.key }
        return Boxed {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(provider.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                    Text("\(provider.cli)" + (provider.program == nil ? ", not found on the PATH" : "") + " · \(login)").font(.system(size: 12)).foregroundStyle(provider.program == nil ? Theme.redText : Theme.secondary)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 14).padding(.vertical, 10).frame(maxWidth: .infinity, alignment: .leading).background(Theme.band)
                ForEach(provider.tiers) { tier in
                    Rectangle().fill(Theme.hairline).frame(height: 1)
                    tierRow(provider, tier)
                }
                ForEach(added, id: \.key) { key, value in
                    Rectangle().fill(Theme.hairline).frame(height: 1)
                    HStack(spacing: 12) {
                        Text(String(key.dropFirst(provider.name.count + 1))).font(.system(size: 13, weight: .medium)).frame(width: 110, alignment: .leading)
                        Text(value).font(.system(size: 12.5, design: .monospaced)).frame(width: 240, alignment: .leading)
                        Text("new, not saved").font(.system(size: 12)).foregroundStyle(Theme.blueText)
                        Spacer(minLength: 0)
                        small("✕", "Don't add \(key)") { model?.added[key] = nil }
                    }
                    .foregroundStyle(Theme.text).padding(.horizontal, 14).padding(.vertical, 6)
                }
                if let model {
                    Rectangle().fill(Theme.hairline).frame(height: 1)
                    HStack(spacing: 12) {
                        field("new tier", Binding(get: { model.newTier[provider.name] ?? "" }, set: { model.newTier[provider.name] = $0 }), label: "Name of a new tier of \(provider.name)").frame(width: 110)
                        field("its model", Binding(get: { model.newModel[provider.name] ?? "" }, set: { model.newModel[provider.name] = $0 }), label: "Model of the new tier of \(provider.name)").frame(width: 240)
                        small("Add tier", "Add the new tier to \(provider.name)") { model.addTier(provider.name) }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 14).padding(.vertical, 6)
                }
            }
        }
    }

    private func field(_ placeholder: String, _ text: Binding<String>, label: String) -> some View {
        // The label sits on the field itself, not on its frame, so that the
        // field stays the thing that takes the text.
        TextField(placeholder, text: text).textFieldStyle(.plain).font(.system(size: 12.5, design: .monospaced))
            .accessibilityLabel(label)
            .padding(.horizontal, 8).frame(height: 26)
            .background(Color.white, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.control, lineWidth: 1))
    }

    private func tierRow(_ provider: ProviderInfo, _ tier: TierInfo) -> some View {
        let key = "\(provider.name)/\(tier.tier)"
        let checking = (report.checking ?? []).contains(key)
        let state = tierState(tier, checking: checking, draft: model?.models[key])
        let ink: Color = {
            switch state.kind {
            case "ok": return Theme.greenText
            case "bad": return Theme.redText
            case "busy", "draft": return Theme.blueText
            default: return Theme.amber
            }
        }()
        return HStack(spacing: 12) {
            Text(tier.tier).font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.text).frame(width: 110, alignment: .leading)
            if let model {
                field(tier.model, Binding(get: { model.models[key] ?? tier.model }, set: { model.models[key] = $0 }), label: "Model of \(provider.name) \(tier.tier)").frame(width: 240)
            } else {
                Text(tier.model).font(.system(size: 12.5, design: .monospaced)).foregroundStyle(Theme.text).frame(width: 240, alignment: .leading)
            }
            Text(tier.family ?? "no family").font(.system(size: 12)).foregroundStyle(tier.family == nil ? Theme.redText : Theme.secondary).frame(width: 80, alignment: .leading)
            Text(state.text).font(.system(size: 12)).foregroundStyle(ink).lineLimit(2).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            if model != nil, !checking, state.kind != "draft" {
                small(tier.check == nil ? "Check" : "Check again", "Check \(provider.name) \(tier.tier)") { check(provider.name, tier.tier) }
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 6)
    }
}
