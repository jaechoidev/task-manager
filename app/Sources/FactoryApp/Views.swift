import AppKit
import SwiftUI

// The panes are built from plain stacks, with no List or Table, so the same
// views can be drawn to an image for a check without a window.

extension Color {
    static func forState(_ state: String) -> Color {
        switch state {
        case "needs-a-person": return .red
        case "running": return .blue
        case "waiting": return .orange
        case "pr-open", "passed": return .purple
        case "merged": return .green
        default: return .gray
        }
    }

    static func forKind(_ kind: String) -> Color {
        switch kind {
        case "agent": return .blue
        case "check": return .teal
        case "engine": return .gray
        default: return .purple
        }
    }
}

struct StateDot: View {
    var state: String
    var body: some View {
        Circle().fill(Color.forState(state)).frame(width: 9, height: 9)
    }
}

struct IssueRowView: View {
    var issue: Issue
    var selected: Bool
    /// Says which repository the issue is in. Two projects both have a #4.
    var showRepo = false

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            StateDot(state: issue.state).padding(.top, 5)
            VStack(alignment: .leading, spacing: 2) {
                Text("#\(issue.number) \(issue.title)").font(.body).lineLimit(2)
                Text(stateCaption(issue, showRepo: showRepo))
                    .font(.caption).foregroundStyle(.secondary)
                if !issue.waitsOn.isEmpty {
                    Text(issue.waitsOn).font(.caption).foregroundStyle(Color.forState(issue.state)).lineLimit(2)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 6).padding(.horizontal, 8)
        .background(selected ? Color.accentColor.opacity(0.18) : .clear, in: RoundedRectangle(cornerRadius: 6))
        .contentShape(Rectangle())
    }
}

/// Every issue the engine knows, grouped by what a person needs first.
struct IssueListPane: View {
    var issues: [Issue]
    var selected: String?
    var select: (String) -> Void = { _ in }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if issues.isEmpty {
                Text("No issue has been run yet.").foregroundStyle(.secondary).padding(8)
            }
            let several = Set(issues.map(\.repo)).count > 1
            ForEach(grouped(issues), id: \.group) { section in
                Text(section.group.rawValue.uppercased())
                    .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                    .padding(.top, 10).padding(.horizontal, 8)
                ForEach(section.issues) { issue in
                    IssueRowView(issue: issue, selected: issue.id == selected, showRepo: several)
                        .onTapGesture { select(issue.id) }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct Card<Content: View>: View {
    var title: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline)
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(Color.gray.opacity(0.10), in: RoundedRectangle(cornerRadius: 8))
    }
}

/// One issue: where it stands, what a person should look at, each step, and
/// what the running agent or check is printing.
struct IssueDetailPane: View {
    var issue: Issue
    var detail: IssueDetail?
    var log: LogTail?
    var retry: () -> Void = {}
    var drop: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("\(issue.repo) #\(issue.number)").font(.caption).foregroundStyle(.secondary)
                Text(issue.title).font(.title2.weight(.semibold))
                HStack(spacing: 6) {
                    StateDot(state: issue.state)
                    Text(stateLabel(issue)).font(.callout)
                    if issue.pass > 1 { Text("pass \(issue.pass)").font(.callout).foregroundStyle(.secondary) }
                    if let url = issue.prUrl, let link = URL(string: url) {
                        // Plain text with a tap, not a Link: a Link can't be
                        // drawn to an image, and this pane is.
                        Text("Pull request #\(link.lastPathComponent)").font(.callout).foregroundStyle(Color.accentColor).underline()
                            .onTapGesture { NSWorkspace.shared.open(link) }
                    }
                }
                if !issue.waitsOn.isEmpty && issue.state != "needs-a-person" {
                    Text("Waiting on: \(issue.waitsOn)").font(.callout).foregroundStyle(.orange)
                }
            }

            if issue.state == "needs-a-person" {
                Card(title: "Needs you") {
                    Text(detail?.failure ?? issue.waitsOn).font(.callout).lineLimit(8)
                    if let judgment = issue.judgment {
                        Text("Judgment: \(judgment)").font(.callout).foregroundStyle(.secondary).lineLimit(10)
                    }
                    HStack {
                        Button("Start over", action: retry)
                        Button("Take off the queue", role: .destructive, action: drop)
                    }
                }
            }

            if let summary = detail?.summary {
                Card(title: "Summary") {
                    Text(summary.whatChanged).font(.callout)
                    Text(summary.howItWent).font(.callout).foregroundStyle(.secondary)
                    if !summary.forAHuman.isEmpty {
                        Text("To look at").font(.subheadline.weight(.semibold)).padding(.top, 2)
                        ForEach(Array(summary.forAHuman.enumerated()), id: \.offset) { _, item in
                            Text("• " + item).font(.callout)
                        }
                    }
                }
            }

            if let steps = detail?.steps, !steps.isEmpty {
                Card(title: "Steps") {
                    Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 3) {
                        GridRow {
                            Text("Pass"); Text("Step"); Text("Who"); Text("Result"); Text("Time")
                        }
                        .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                        ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
                            GridRow {
                                Text("\(step.pass)")
                                Text(step.step)
                                Text(stepWho(step)).foregroundStyle(.secondary)
                                Text(step.verdict).foregroundStyle(verdictColor(step.verdict))
                                Text(duration(step.seconds)).foregroundStyle(.secondary)
                            }
                            .font(.callout.monospacedDigit())
                        }
                    }
                }
            }

            if let log, !log.text.isEmpty {
                Card(title: "Latest output" + (log.file.map { ": \($0)" } ?? "")) {
                    Text(logExcerpt(log.text))
                        .font(.caption.monospaced())
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func verdictColor(_ verdict: String) -> Color {
        switch verdict {
        case "fail", "invalid", "failed", "changes", "timeout", "no-verdict", "conflict", "refused": return .red
        case "rate-limited", "skipped": return .orange
        default: return .primary
        }
    }
}

/// The end of a log, cut to fit a card. An agent's raw reply is one very long
/// line of JSON, so long lines are cut too.
func logExcerpt(_ text: String, lines: Int = 14, width: Int = 180) -> String {
    text.split(separator: "\n", omittingEmptySubsequences: true)
        .suffix(lines)
        .map { $0.count > width ? String($0.prefix(width)) + " ..." : String($0) }
        .joined(separator: "\n")
}

/// The repository's workflow as a graph. Read only.
struct WorkflowPane: View {
    var workflow: Workflow

    var body: some View {
        let layout = GraphLayout.make(workflow)
        VStack(alignment: .leading, spacing: 10) {
            Text("Workflow on \(workflow.branch)").font(.headline)
            ZStack(alignment: .topLeading) {
                Canvas { context, _ in
                    // Forward arrows between neighbours.
                    for index in layout.nodes.indices.dropLast() {
                        let a = layout.nodes[index].frame, b = layout.nodes[index + 1].frame
                        var path = Path()
                        path.move(to: CGPoint(x: a.maxX, y: a.midY))
                        path.addLine(to: CGPoint(x: b.minX, y: b.midY))
                        context.stroke(path, with: .color(.secondary), lineWidth: 1.2)
                        context.fill(arrowHead(at: CGPoint(x: b.minX, y: b.midY), pointing: .zero), with: .color(.secondary))
                    }
                    // A failure goes back to the step that fixes it.
                    for edge in layout.backEdges {
                        let from = layout.nodes[edge.from].frame, to = layout.nodes[edge.to].frame
                        let y = from.minY - edge.lift
                        var path = Path()
                        path.move(to: CGPoint(x: from.midX, y: from.minY))
                        path.addLine(to: CGPoint(x: from.midX, y: y))
                        path.addLine(to: CGPoint(x: to.midX + 10, y: y))
                        path.addLine(to: CGPoint(x: to.midX + 10, y: to.minY))
                        context.stroke(path, with: .color(.red.opacity(0.7)), style: StrokeStyle(lineWidth: 1.2, dash: [4, 3]))
                        context.fill(arrowHead(at: CGPoint(x: to.midX + 10, y: to.minY), pointing: CGPoint(x: 0, y: 1)), with: .color(.red.opacity(0.8)))
                    }
                }
                .frame(width: layout.size.width, height: layout.size.height)
                ForEach(Array(layout.nodes.enumerated()), id: \.offset) { _, node in
                    VStack(spacing: 1) {
                        Text(node.step.name).font(.callout.weight(.semibold)).lineLimit(1)
                        Text(node.step.vendor ?? (node.step.kind == "engine" ? "engine" : node.step.kind))
                            .font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                    }
                    .frame(width: node.frame.width, height: node.frame.height)
                    .background(Color.forKind(node.step.kind).opacity(0.18), in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.forKind(node.step.kind).opacity(0.7), lineWidth: 1))
                    .offset(x: node.frame.minX, y: node.frame.minY)
                }
            }
            .frame(width: layout.size.width, height: layout.size.height, alignment: .topLeading)

            HStack(spacing: 14) {
                legend("agent", "An LLM call")
                legend("check", "A command")
                legend("engine", "The engine itself")
                HStack(spacing: 4) {
                    Rectangle().fill(.red.opacity(0.7)).frame(width: 16, height: 1.5)
                    Text("A failure goes back").font(.caption).foregroundStyle(.secondary)
                }
            }
            Text("Fix rounds: \(workflow.limits.fixRounds) before the pull request, \(workflow.limits.githubFixRounds) on GitHub. New attempts a judgment may start: \(workflow.limits.judgments).")
                .font(.caption).foregroundStyle(.secondary)
            if let judgment = workflow.judgment {
                Text("When stuck: \(judgment.module) on \(judgment.vendor ?? "?").").font(.caption).foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func legend(_ kind: String, _ text: String) -> some View {
        HStack(spacing: 4) {
            RoundedRectangle(cornerRadius: 3).fill(Color.forKind(kind).opacity(0.35)).frame(width: 12, height: 12)
            Text(text).font(.caption).foregroundStyle(.secondary)
        }
    }

    /// A small triangle. `pointing` (0,0) means to the right, (0,1) means down.
    private func arrowHead(at tip: CGPoint, pointing: CGPoint) -> Path {
        var path = Path()
        path.move(to: tip)
        if pointing.y > 0 {
            path.addLine(to: CGPoint(x: tip.x - 4, y: tip.y - 7))
            path.addLine(to: CGPoint(x: tip.x + 4, y: tip.y - 7))
        } else {
            path.addLine(to: CGPoint(x: tip.x - 7, y: tip.y - 4))
            path.addLine(to: CGPoint(x: tip.x - 7, y: tip.y + 4))
        }
        path.closeSubpath()
        return path
    }
}

/// What runs where: each scheduler's places and who has them, what waits and
/// what for, each pool's slots, and who is out of quota.
struct SchedulePane: View {
    var schedule: Schedule
    /// Opens an issue, by its id.
    var open: (String) -> Void = { _ in }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if schedule.schedulers.isEmpty {
                Card(title: "Scheduler") {
                    Text("No scheduler is running. `factory schedule` starts one over the registered projects.")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
            ForEach(schedule.schedulers) { board in
                Card(title: "Places") {
                    Text(placesLine(board)).font(.callout)
                    if let reason = board.heldBack {
                        Text("Starting nothing further: \(reason)").font(.callout).foregroundStyle(.orange)
                    }
                    ForEach(board.projects) { project in
                        projectSection(project)
                    }
                }
            }
            if !schedule.pools.isEmpty {
                Card(title: "Pools") {
                    Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 3) {
                        GridRow {
                            Text("Pool"); Text("Slot"); Text("Held by"); Text("For"); Text("Time")
                        }
                        .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                        ForEach(schedule.pools) { pool in
                            ForEach(pool.slots) { slot in
                                GridRow {
                                    Text(slot.slot == 1 ? pool.name : "")
                                    Text("\(slot.slot) of \(pool.size)").foregroundStyle(.secondary)
                                    if let holder = slot.holder {
                                        Text("\(shortRepo(holder.repo)) #\(holder.number)")
                                        Text(holder.what)
                                        Text(duration(Double(max(0, schedule.now - holder.since)))).foregroundStyle(.secondary)
                                    } else {
                                        Text("free").foregroundStyle(.secondary)
                                        Text("")
                                        Text("")
                                    }
                                }
                                .font(.callout.monospacedDigit())
                            }
                        }
                    }
                }
            }
            if !schedule.out.isEmpty || !schedule.quota.isEmpty {
                Card(title: "Quota") {
                    if schedule.out.isEmpty {
                        Text("Nobody is set aside for a usage limit.").font(.callout).foregroundStyle(.secondary)
                    }
                    ForEach(schedule.out) { entry in
                        VStack(alignment: .leading, spacing: 1) {
                            Text("\(entry.who) is set aside until \(clockTime(entry.until))" + (entry.hits > 1 ? ", after \(entry.hits) limits in a row" : ""))
                                .font(.callout).foregroundStyle(.orange)
                            Text(entry.reason).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                        }
                    }
                    ForEach(schedule.quota) { reading in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(reading.provider + (reading.outUntil.map { ": used up until \(clockTime($0))" } ?? "") + ", read at \(clockTime(reading.at))")
                                .font(.callout.weight(.semibold)).foregroundStyle(reading.outUntil == nil ? Color.primary : Color.orange)
                            ForEach(reading.windows, id: \.self) { window in
                                Text(window).font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func projectSection(_ project: ProjectBoard) -> some View {
        let limit = project.maxParallel.map { ", at most \($0) at once" } ?? ""
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(project.repo).font(.subheadline.weight(.semibold))
                Text("issues labelled '\(project.label)'\(limit)").font(.caption).foregroundStyle(.secondary)
            }
            if project.running.isEmpty && project.waiting.isEmpty && project.parked.isEmpty {
                Text("Nothing is running or waiting.").font(.callout).foregroundStyle(.secondary)
            }
            ForEach(project.running) { entry in
                row(entry, in: project, state: entry.state == "waiting" ? "waiting" : "running", detail: runningLine(entry, now: schedule.now))
            }
            ForEach(project.parked) { entry in
                let until = entry.until.map { "Parked until \(clockTime($0))" } ?? "Parked"
                row(entry, in: project, state: "waiting", detail: until + ((entry.waitsOn ?? "").isEmpty ? "" : ": \(entry.waitsOn ?? "")"))
            }
            ForEach(project.waiting) { entry in
                row(entry, in: project, state: "unknown", detail: "Not started. Waits on \(entry.waitsOn ?? "a free place")")
            }
            let ended = [("Merged", project.merged), ("Pull request open", project.opened), ("Blocked", project.failed)]
                .filter { !$0.1.isEmpty }
                .map { "\($0.0): " + $0.1.map { "#\($0)" }.joined(separator: ", ") }
            if !ended.isEmpty {
                Text(ended.joined(separator: ". ")).font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(.top, 6)
    }

    private func row(_ entry: BoardEntry, in project: ProjectBoard, state: String, detail: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            StateDot(state: state).padding(.top, 5)
            VStack(alignment: .leading, spacing: 1) {
                Text("#\(entry.number) \(entry.title)").font(.callout).lineLimit(1)
                Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(2)
            }
            Spacer(minLength: 0)
        }
        .contentShape(Rectangle())
        .onTapGesture { open("\(project.repo)#\(entry.number)") }
    }
}

/// Calls and time per vendor.
struct UsagePane: View {
    var usage: [UsageRow]

    var body: some View {
        Card(title: "Usage by vendor") {
            if usage.isEmpty {
                Text("No agent has been called yet.").font(.callout).foregroundStyle(.secondary)
            }
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 3) {
                GridRow {
                    Text("Vendor"); Text("Calls"); Text("Time"); Text("Usage limits hit")
                }
                .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                ForEach(usage) { row in
                    GridRow {
                        Text(row.vendor)
                        Text("\(row.calls)")
                        Text(duration(row.seconds))
                        Text("\(row.rateLimited)").foregroundStyle(row.rateLimited > 0 ? .orange : .primary)
                    }
                    .font(.callout.monospacedDigit())
                }
            }
        }
    }
}
