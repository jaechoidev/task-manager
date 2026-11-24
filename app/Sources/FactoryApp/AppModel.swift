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
    @Published var repos: [String] = []
    /// Why the engine can't be reached, when it can't.
    @Published var problem: String?
    /// What the last retry or drop did.
    @Published var notice: String?

    @AppStorage("address") var address = "http://127.0.0.1:7878"
    @AppStorage("token") var token = ""

    private var loop: Task<Void, Never>?
    private var knownStuck: Set<String> = []

    var client: APIClient { APIClient(address: address, token: token) }
    var needingAPerson: [Issue] { issues.filter { $0.state == "needs-a-person" } }
    var active: [Issue] { issues.filter { $0.state == "running" || $0.state == "waiting" } }
    var selectedIssue: Issue? { issues.first { $0.id == selected } }

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
            if let repo = selectedIssue?.repo ?? health.repos.first, health.repos.contains(repo) {
                let query = repo.addingPercentEncoding(withAllowedCharacters: .urlQueryValueAllowed) ?? repo
                self.workflow = try? await client.get("workflow?repo=\(query)")
            } else {
                self.workflow = nil
            }
        } catch {
            self.problem = "Can't reach the engine at \(address). Start it with `factory serve --repo-dir <checkout>`. (\(error.localizedDescription))"
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
