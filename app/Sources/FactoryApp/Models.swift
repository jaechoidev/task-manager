import Foundation

// What the engine's API returns. The engine sends data, and everything about
// how it looks is decided here.

struct Issue: Codable, Identifiable, Equatable {
    var repo: String
    var number: Int
    var key: String
    var title: String
    /// `running`, `waiting`, `needs-a-person`, `pr-open`, `merged`, `passed`, `dropped` or `unknown`.
    var state: String
    var stage: String
    var status: String
    var waitsOn: String
    var pass: Int
    var runId: String
    var headSha: String?
    var prUrl: String?
    var judgment: String?
    var updatedAt: String

    var id: String { "\(repo)#\(number)" }
    var path: String { "issues/\(repo)/\(number)" }
}

struct StepRow: Codable, Equatable {
    var step: String
    /// Who answered an agent step: the provider, or the vendor.
    var vendor: String?
    /// The model it was started with. Missing for steps from before the engine kept it.
    var model: String?
    var attempt: Int
    var verdict: String
    var exitCode: Int?
    var seconds: Double
    var pass: Int
}

struct Summary: Codable, Equatable {
    var whatChanged: String
    var howItWent: String
    var forAHuman: [String]
}

struct IssueDetail: Codable, Equatable {
    var repo: String
    var number: Int
    var title: String
    var state: String
    var stage: String
    var waitsOn: String
    var pass: Int
    var prUrl: String?
    var headSha: String?
    var judgment: String?
    var failure: String?
    var summary: Summary?
    var steps: [StepRow]
    var runDir: String
    var files: [String]
}

struct LogTail: Codable, Equatable {
    var file: String?
    var text: String
}

struct WorkflowStep: Codable, Equatable {
    var name: String
    var module: String
    /// `agent`, `check` or `engine`.
    var kind: String
    var vendor: String?
    var locks: [String]
}

struct WorkflowLimits: Codable, Equatable {
    var fixRounds: Int
    var githubFixRounds: Int
    var judgments: Int
}

struct Workflow: Codable, Equatable {
    var branch: String
    var steps: [WorkflowStep]
    var onFail: [String: String]
    var judgment: WorkflowStep?
    var summary: WorkflowStep?
    var limits: WorkflowLimits
    /// The workflow file itself. Present for the one on the default branch.
    var text: String?
}

/// A module a workflow may name.
struct ModuleInfo: Codable, Equatable, Identifiable {
    var module: String
    var kind: String
    var role: String?
    var needs: [String]
    var produces: [String]
    var locks: [String]
    /// `built-in`, `repository`, or `repository, overriding the built-in`.
    var source: String
    var problem: String?
    var id: String { module }
}

/// What the engine says about a draft workflow.
struct Validation: Codable, Equatable {
    var ok: Bool
    var errors: [String]
    var workflow: Workflow?
}

/// What came of asking for a pull request.
struct Proposal: Codable, Equatable {
    var ok: Bool
    var url: String?
    var errors: [String]?
    var error: String?
    var files: [String]?

    var problems: [String] { (errors ?? []) + (error.map { [$0] } ?? []) }
}

/// Adds a step to a workflow file's text: a line at the end of the `workflow:`
/// list, and for a check the `on_fail` line every check needs. The person
/// edits from there. The file stays the source of truth.
func insertStep(into text: String, module: ModuleInfo, developer: String?) -> String {
    let name = module.module.split(separator: "/").last.map(String.init) ?? module.module
    let vendor = module.kind == "check" ? "" : ", vendor: claude"
    let step = "  - \(name): { module: \(module.module)\(vendor) }"
    var lines = text.components(separatedBy: "\n")
    // The step list ends at the next top-level key after `workflow:`.
    guard let start = lines.firstIndex(where: { $0.hasPrefix("workflow:") }) else { return text }
    let end = lines[(start + 1)...].firstIndex(where: { line in
        guard let first = line.first else { return false }
        return first != " " && first != "#" && first != "-"
    }) ?? lines.count
    var at = end
    while at > start + 1 && lines[at - 1].trimmingCharacters(in: .whitespaces).isEmpty { at -= 1 }
    lines.insert(step, at: at)
    if module.kind == "check", let developer, let onFail = lines.firstIndex(where: { $0.hasPrefix("on_fail:") }) {
        lines.insert("  \(name): \(developer)", at: onFail + 1)
    }
    return lines.joined(separator: "\n")
}

/// What `GET /v1/schedule` answers: who has each place and each slot, and
/// who is out of quota. Times are seconds since 1970, and `now` is the
/// engine's clock, so nothing here depends on this machine's.
struct Schedule: Codable, Equatable {
    var now: Int
    var schedulers: [SchedulerBoard]
    var pools: [Pool]
    var out: [OutOfQuota]
    var quota: [QuotaReading]
}

/// One running `factory schedule` or `factory queue`.
struct SchedulerBoard: Codable, Equatable, Identifiable {
    var pid: Int
    var startedAt: Int
    var updatedAt: Int
    var places: Int
    var inOrder: Bool
    var watch: Bool
    /// Why nothing further is started although a place is free.
    var heldBack: String?
    var projects: [ProjectBoard]

    var id: Int { pid }
    var inUse: Int { projects.reduce(0) { $0 + $1.running.count } }
}

struct ProjectBoard: Codable, Equatable, Identifiable {
    var repo: String
    var label: String
    var maxParallel: Int?
    var running: [BoardEntry]
    var waiting: [BoardEntry]
    var parked: [BoardEntry]
    var merged: [Int]
    var opened: [Int]
    var failed: [Int]

    var id: String { repo }
}

/// An issue on a scheduler's board. Which fields are set depends on the list
/// it is in: `since` for a running one, `until` for a parked one.
struct BoardEntry: Codable, Equatable, Identifiable {
    var number: Int
    var key: String
    var title: String
    var since: Int?
    var until: Int?
    var waitsOn: String?
    /// From the database, for an issue that has started.
    var state: String?
    var stage: String?
    var pass: Int?

    var id: Int { number }
}

struct Pool: Codable, Equatable, Identifiable {
    var name: String
    var size: Int
    var slots: [Slot]
    var id: String { name }
    var held: Int { slots.filter { $0.holder != nil }.count }
}

struct Slot: Codable, Equatable, Identifiable {
    var slot: Int
    var holder: Holder?
    var id: Int { slot }
}

struct Holder: Codable, Equatable {
    var repo: String
    var number: Int
    /// The step, and for an agent call the model: `verify`, or `review, opus`.
    var what: String
    var since: Int
}

struct OutOfQuota: Codable, Equatable, Identifiable {
    /// `provider/model`.
    var who: String
    var until: Int
    var hits: Int
    var reason: String
    var id: String { who }
}

struct QuotaReading: Codable, Equatable, Identifiable {
    var provider: String
    var at: Int
    var windows: [String]
    var outUntil: Int?
    var id: String { provider }
}

/// `todo-trial` of `owner/todo-trial`.
func shortRepo(_ repo: String) -> String {
    repo.split(separator: "/").last.map(String.init) ?? repo
}

/// How a scheduler hands out its places, in one line.
func placesLine(_ board: SchedulerBoard) -> String {
    let places = board.places == 1 ? "1 place" : "\(board.places) places"
    let how = board.projects.count < 2 ? "" : (board.inOrder ? ", one project after the other" : ", projects side by side")
    let watch = board.watch ? ", watching for new work" : ""
    return "\(places), \(board.inUse) in use\(how)\(watch)"
}

/// What a running issue is doing and for how long it has had its place.
func runningLine(_ entry: BoardEntry, now: Int) -> String {
    var parts: [String] = []
    if let stage = entry.stage {
        let waiting = entry.state == "waiting" && !(entry.waitsOn ?? "").isEmpty
        parts.append(waiting ? "waiting at \(stage) for \(entry.waitsOn ?? "")" : stage)
    } else {
        parts.append("starting")
    }
    if let pass = entry.pass, pass > 1 { parts.append("pass \(pass)") }
    if let since = entry.since { parts.append(duration(Double(max(0, now - since)))) }
    return parts.joined(separator: " · ")
}

/// Who answered a step: `glm · haiku`, `codex`, or the engine itself.
func stepWho(_ step: StepRow) -> String {
    guard let vendor = step.vendor else { return "engine" }
    guard let model = step.model, !model.isEmpty else { return vendor }
    return "\(vendor) · \(model)"
}

/// A time of day on this machine's clock, such as `14:05`.
func clockTime(_ seconds: Int, in zone: TimeZone = .current) -> String {
    let formatter = DateFormatter()
    formatter.dateFormat = "HH:mm"
    formatter.timeZone = zone
    return formatter.string(from: Date(timeIntervalSince1970: TimeInterval(seconds)))
}

struct UsageRow: Codable, Equatable, Identifiable {
    var vendor: String
    var calls: Int
    var seconds: Double
    var rateLimited: Int
    /// Tokens as each program counted them for its own calls: all input,
    /// the part of it read from a cache, and the output. Missing from an
    /// engine that does not keep them.
    var tokensIn: Int?
    var tokensCached: Int?
    var tokensOut: Int?
    /// How many of the calls have a count. A call that failed early has none.
    var tokenCalls: Int?
    var id: String { vendor }

    /// Input and output together, when the engine counted any.
    var tokens: Int? {
        guard let tokensIn, let tokensOut, (tokenCalls ?? 0) > 0 else { return nil }
        return tokensIn + tokensOut
    }
}

/// A token count in a few characters: `812`, `45.3k`, `2.1M`.
func tokenCount(_ count: Int) -> String {
    let value = Double(count)
    if count < 1_000 { return "\(count)" }
    if count < 999_500 { return String(format: value < 99_950 ? "%.1fk" : "%.0fk", value / 1_000) }
    return String(format: value < 99_950_000 ? "%.1fM" : "%.0fM", value / 1_000_000)
}

/// What a vendor's tokens were made of: `2.1M in (1.9M cached) · 34.0k out`.
func tokenDetail(_ row: UsageRow) -> String {
    guard let input = row.tokensIn, let output = row.tokensOut else { return "" }
    let cached = (row.tokensCached ?? 0) > 0 ? " (\(tokenCount(row.tokensCached ?? 0)) cached)" : ""
    return "\(tokenCount(input)) in\(cached) · \(tokenCount(output)) out"
}

struct Health: Codable {
    var ok: Bool
    var version: String
    var repos: [String]
}

extension JSONDecoder {
    /// The API uses snake_case names.
    static var api: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }
}

/// How the list groups issues, in the order a person needs them.
enum IssueGroup: String, CaseIterable {
    case needsAPerson = "Needs you"
    case active = "Running and waiting"
    case open = "Pull request open"
    case done = "Done"

    static func of(_ issue: Issue) -> IssueGroup {
        switch issue.state {
        case "needs-a-person": return .needsAPerson
        case "running", "waiting", "unknown": return .active
        case "pr-open", "passed": return .open
        default: return .done
        }
    }
}

func grouped(_ issues: [Issue]) -> [(group: IssueGroup, issues: [Issue])] {
    IssueGroup.allCases.compactMap { group in
        let members = issues.filter { IssueGroup.of($0) == group }
        return members.isEmpty ? nil : (group, members)
    }
}

/// A short phrase for an issue's state.
func stateLabel(_ issue: Issue) -> String {
    switch issue.state {
    case "running": return "Running \(issue.stage)"
    case "waiting": return "Waiting at \(issue.stage)"
    case "needs-a-person": return "Stopped at \(issue.stage)"
    case "pr-open": return "Pull request open"
    case "merged": return "Merged"
    case "passed": return "Passed"
    case "dropped": return "Dropped"
    default: return issue.stage
    }
}

/// The line under an issue's title in the list: where it stands, and which
/// repository it is in when the list holds more than one.
func stateCaption(_ issue: Issue, showRepo: Bool) -> String {
    let state = stateLabel(issue) + (issue.pass > 1 ? ", pass \(issue.pass)" : "")
    return showRepo ? "\(shortRepo(issue.repo)) · \(state)" : state
}

func duration(_ seconds: Double) -> String {
    if seconds < 1 { return "under 1 s" }
    if seconds < 90 { return "\(Int(seconds.rounded())) s" }
    return "\(Int(seconds / 60)) min \(Int(seconds) % 60) s"
}

// MARK: - What the redesigned screens are built from

/// Which mark an issue gets. An issue the scheduler has parked is waiting
/// for quota, which reads differently from waiting inside a step.
func mark(of issue: Issue, parked: Bool = false) -> Mark {
    switch issue.state {
    case "needs-a-person": return .needs
    case "running": return .running
    case "waiting": return parked ? .parked : .waiting
    case "pr-open", "passed": return .pullRequest
    case "merged": return .done
    case "dropped": return .dropped
    default: return .notStarted
    }
}

/// Whether an issue matches what was typed into the filter and the project
/// that was picked. Nothing typed and no project picked match everything.
func matches(_ issue: Issue, filter: String, project: String?) -> Bool {
    if let project, issue.repo != project { return false }
    let words = filter.lowercased().split(separator: " ")
    let text = "#\(issue.number) \(issue.title) \(issue.key) \(issue.repo) \(issue.stage)".lowercased()
    return words.allSatisfy { text.contains($0) }
}

/// One step of the workflow as the strip above an issue shows it.
struct StageChip: Equatable, Identifiable {
    var name: String
    /// `ok`, `skip`, `fail`, `current` or `pending`.
    var state: String
    /// How often it ran.
    var times: Int

    var id: String { name }
    var mark: String {
        switch state {
        case "ok": return "✓"
        case "skip": return "–"
        case "fail": return "✕"
        case "current": return "▶"
        default: return ""
        }
    }
    var extra: String {
        if state == "skip" { return " skipped" }
        return times > 1 ? " ×\(times)" : ""
    }
}

let failingVerdicts: Set<String> = ["fail", "invalid", "failed", "changes", "timeout", "no-verdict", "conflict", "refused"]

/// The workflow's steps in order, each with how it went for this issue.
/// `order` is the workflow's own list when it is known. Without it the steps
/// are taken as they ran. `current` is the step an unfinished issue is at.
func stageChips(steps: [StepRow], order: [String]?, current: String?) -> [StageChip] {
    var seen: [String] = []
    for step in steps where !seen.contains(step.step) { seen.append(step.step) }
    let names = order ?? seen
    return names.map { name in
        let runs = steps.filter { $0.step == name }
        let state: String
        if name == current {
            state = "current"
        } else if let last = runs.last {
            state = last.verdict == "skipped" ? "skip" : (failingVerdicts.contains(last.verdict) ? "fail" : "ok")
        } else {
            state = "pending"
        }
        return StageChip(name: name, state: state, times: runs.count)
    }
}

/// The steps of one pass, with a line on why the pass began.
struct PassGroup: Equatable, Identifiable {
    var pass: Int
    var note: String
    var steps: [StepRow]
    var id: Int { pass }
}

func passGroups(_ steps: [StepRow]) -> [PassGroup] {
    var groups: [PassGroup] = []
    for step in steps {
        if let last = groups.indices.last, groups[last].pass == step.pass {
            groups[last].steps.append(step)
        } else {
            // A later pass begins because the base branch came in, or
            // because something failed and went back.
            let before = groups.last?.steps.last?.verdict
            let note = groups.isEmpty ? "" : (before == "merged-base" ? "after the base branch was merged in" : "after a step sent it back")
            groups.append(PassGroup(pass: step.pass, note: note, steps: [step]))
        }
    }
    for index in groups.indices where groups[index].note.isEmpty {
        let count = groups[index].steps.count
        groups[index].note = "\(count) step\(count == 1 ? "" : "s")"
    }
    return groups
}

/// A line of a quota reading, such as `5-hour window: 43% used, resets ...`,
/// taken apart for a bar. A line that is not a window, such as `plan: pro`,
/// has no share.
struct QuotaLine: Equatable {
    var name: String
    /// 0 to 1, when the line says how much is used.
    var share: Double?
    var used: String
    var detail: String
}

func quotaLine(_ line: String) -> QuotaLine {
    guard let colon = line.range(of: ": ") else { return QuotaLine(name: line, share: nil, used: "", detail: "") }
    let name = String(line[..<colon.lowerBound])
    let rest = String(line[colon.upperBound...])
    let parts = rest.components(separatedBy: ", ")
    let used = parts.first ?? rest
    var share: Double?
    if used.hasSuffix("% used"), let percent = Double(used.dropLast("% used".count)) {
        share = percent / 100
    }
    return QuotaLine(name: name, share: share, used: used, detail: parts.dropFirst().joined(separator: ", "))
}

/// The line of a workflow file a problem is about, counted from 1, when the
/// problem names a step: `step 'verify': ...` is about the line `- verify:`.
func problemLine(_ problem: String, in text: String) -> Int? {
    guard let open = problem.range(of: "step '"), let close = problem[open.upperBound...].firstIndex(of: "'") else { return nil }
    let name = problem[open.upperBound..<close]
    let lines = text.components(separatedBy: "\n")
    return lines.firstIndex { $0.trimmingCharacters(in: .whitespaces).hasPrefix("- \(name):") }.map { $0 + 1 }
}

/// The step a problem names, if it names one.
func problemStep(_ problem: String) -> String? {
    guard let open = problem.range(of: "step '"), let close = problem[open.upperBound...].firstIndex(of: "'") else { return nil }
    return String(problem[open.upperBound..<close])
}

/// How long ago the engine last answered, in words.
func agoLine(_ seconds: Int) -> String {
    if seconds < 1 { return "updated just now" }
    return "updated \(duration(Double(seconds))) ago"
}
