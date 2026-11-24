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
    var vendor: String?
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

struct UsageRow: Codable, Equatable, Identifiable {
    var vendor: String
    var calls: Int
    var seconds: Double
    var rateLimited: Int
    var id: String { vendor }
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
    case needsAPerson = "Needs a person"
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

func duration(_ seconds: Double) -> String {
    if seconds < 1 { return "under 1 s" }
    if seconds < 90 { return "\(Int(seconds.rounded())) s" }
    return "\(Int(seconds / 60)) min \(Int(seconds) % 60) s"
}
