import Foundation
import SwiftUI

/// Everything the windows show. It asks the engine again every two seconds.
/// The engine has no event stream yet.
@MainActor
final class AppModel: ObservableObject {
    @Published var issues: [Issue] = []
    @Published var selected: String?
    @Published var detail: IssueDetail?
    @Published var log: LogTail?
    @Published var workflow: Workflow?
    @Published var usage: [UsageRow] = []
    /// Who has each place and each slot. Nil when the engine is older than this.
    @Published var schedule: Schedule?
    @Published var repos: [String] = []
    /// Why the engine can't be reached, when it can't.
    @Published var problem: String?
    /// What the last retry or drop did.
    @Published var notice: String?
    /// When the engine last answered.
    @Published var updatedAt: Date?
    /// Only this repository's issues are listed. Nil lists every project's.
    @Published var project: String?

    @AppStorage("address") var address = "http://127.0.0.1:7878"
    @AppStorage("token") var token = ""

    /// The "to look at" items a person has ticked. Kept on this machine only.
    @Published var looked: Set<String> = Set(UserDefaults.standard.stringArray(forKey: "looked") ?? [])

    func toggleLooked(_ key: String) {
        if looked.contains(key) { looked.remove(key) } else { looked.insert(key) }
        UserDefaults.standard.set(Array(looked).sorted(), forKey: "looked")
    }

    /// Until when the Quota heading says the providers are being asked.
    @Published var quotaAskedUntil: Date?
    /// The newest reading there was when the providers were asked.
    private var quotaSeenAt = 0

    var quotaRefreshing: Bool {
        guard let until = quotaAskedUntil, until > Date() else { return false }
        // Done as soon as every reading is newer than when it was asked.
        let oldest = schedule?.quota.map(\.at).min() ?? 0
        return oldest <= quotaSeenAt
    }

    /// Asks the engine to read every provider's plan again.
    func refreshQuota() async {
        quotaSeenAt = schedule?.quota.map(\.at).max() ?? 0
        do {
            try await client.post("quota/refresh")
            quotaAskedUntil = Date().addingTimeInterval(90)
        } catch {
            notice = "Could not ask the providers: \(error.localizedDescription)"
        }
        await refresh()
    }

    private var loop: Task<Void, Never>?
    private var knownStuck: Set<String> = []

    var client: APIClient { APIClient(address: address, token: token) }
    var needingAPerson: [Issue] { issues.filter { $0.state == "needs-a-person" } }
    var active: [Issue] { issues.filter { $0.state == "running" || $0.state == "waiting" } }
    var selectedIssue: Issue? { issues.first { $0.id == selected } }
    /// Every repository there is something to show for.
    var projects: [String] { Array(Set(repos + issues.map(\.repo))).sorted() }
    /// The repository whose workflow is shown.
    var workflowRepo: String? { [project, selectedIssue?.repo, repos.first].compactMap { $0 }.first { repos.contains($0) } }
    /// Issues a scheduler has parked until somebody has quota.
    var parked: Set<String> {
        Set((schedule?.schedulers ?? []).flatMap { board in board.projects.flatMap { project in project.parked.map { "\(project.repo)#\($0.number)" } } })
    }

    func start() {
        loop?.cancel()
        loop = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refresh()
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    func refresh() async {
        let client = self.client
        do {
            let health: Health = try await client.get("health")
            let issues: [Issue] = try await client.get("issues")
            self.repos = health.repos
            self.issues = issues
            self.problem = nil
            self.updatedAt = Date()
            if let project, !health.repos.contains(project), !issues.contains(where: { $0.repo == project }) {
                self.project = nil
            }
            if selected == nil || !issues.contains(where: { $0.id == selected }) {
                selected = (needingAPerson.first ?? active.first ?? issues.first)?.id
            }
            noticeNewlyStuck()
            if let issue = selectedIssue {
                self.detail = try? await client.get(issue.path)
                self.log = try? await client.get(issue.path + "/log")
            } else {
                self.detail = nil
                self.log = nil
            }
            self.usage = (try? await client.get("usage")) ?? []
            self.schedule = try? await client.get("schedule")
            if let repo = project ?? selectedIssue?.repo ?? health.repos.first, health.repos.contains(repo) {
                let query = repo.addingPercentEncoding(withAllowedCharacters: .urlQueryValueAllowed) ?? repo
                self.workflow = try? await client.get("workflow?repo=\(query)")
            } else {
                self.workflow = nil
            }
        } catch {
            self.problem = "Can't reach the engine at \(address). Start it with `factory serve`. (\(error.localizedDescription))"
        }
    }

    /// Tells the person once when an issue newly needs them.
    private func noticeNewlyStuck() {
        let stuck = Set(needingAPerson.map(\.id))
        let new = stuck.subtracting(knownStuck)
        knownStuck = stuck
        for id in new {
            if let issue = issues.first(where: { $0.id == id }) {
                Notifier.post(title: "\(id) needs you", body: "\(issue.title): stopped at \(issue.stage). \(issue.waitsOn)")
            }
        }
    }

    func act(_ action: String, on issue: Issue) async {
        do {
            try await client.post("\(issue.path)/\(action)")
            notice = action == "retry" ? "\(issue.id) is starting over." : "\(issue.id) is off the queue."
        } catch {
            notice = "Could not \(action) \(issue.id): \(error.localizedDescription)"
        }
        await refresh()
    }
}

extension CharacterSet {
    static let urlQueryValueAllowed: CharacterSet = {
        var set = CharacterSet.urlQueryAllowed
        set.remove(charactersIn: "/&=+?")
        return set
    }()
}
