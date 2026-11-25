import AppKit
import SwiftUI

// The sidebar and the issue screen. Everything is built from plain stacks
// and shapes, with no List, Table, Link or system control, so the same views
// can be drawn to an image for a check without a window.

/// What the main area shows.
enum Screen: String, CaseIterable {
    case issue, schedule, workflow, usage, editor
}

/// Lays its children out in rows, starting a new row when one is full.
struct Flow: Layout {
    var spacing: CGFloat = 4
    var lineSpacing: CGFloat = 6

    private func rows(_ subviews: Subviews, width: CGFloat) -> [[(index: Int, size: CGSize)]] {
        var rows: [[(index: Int, size: CGSize)]] = [[]]
        var used: CGFloat = 0
        for index in subviews.indices {
            let size = subviews[index].sizeThatFits(.unspecified)
            if used > 0, used + spacing + size.width > width {
                rows.append([])
                used = 0
            }
            rows[rows.count - 1].append((index, size))
            used += (used > 0 ? spacing : 0) + size.width
        }
        return rows
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? .infinity
        let rows = rows(subviews, width: width)
        let height = rows.map { $0.map(\.size.height).max() ?? 0 }.reduce(0, +) + CGFloat(max(0, rows.count - 1)) * lineSpacing
        let widest = rows.map { $0.map(\.size.width).reduce(0, +) + CGFloat(max(0, $0.count - 1)) * spacing }.max() ?? 0
        return CGSize(width: proposal.width ?? widest, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in rows(subviews, width: bounds.width) {
            let height = row.map(\.size.height).max() ?? 0
            var x = bounds.minX
            for item in row {
                subviews[item.index].place(at: CGPoint(x: x, y: y + (height - item.size.height) / 2), proposal: ProposedViewSize(item.size))
                x += item.size.width + spacing
            }
            y += height + lineSpacing
        }
    }
}

/// The left column: a filter, the three places to go, and every issue.
struct SidebarPane: View {
    /// The issues to list, already narrowed to the filter and the project.
    var issues: [Issue]
    /// Whether rows say which repository an issue is in.
    var several: Bool
    var parked: Set<String> = []
    var selected: String?
    var screen: Screen
    /// Beside "Schedule": how many places are in use, such as `2 of 3`.
    var places: String?
    /// The filter field. Nil draws it without one, for an image.
    var filter: Binding<String>?
    var go: (Screen) -> Void = { _ in }
    var select: (String) -> Void = { _ in }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                if let filter {
                    TextField("Filter issues", text: filter).textFieldStyle(.plain).font(.system(size: 13))
                } else {
                    Text("Filter issues").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                    Spacer(minLength: 0)
                }
            }
            .padding(.horizontal, 8).frame(height: 28)
            .background(Theme.field, in: RoundedRectangle(cornerRadius: 6))
            .padding(.bottom, 8)

            place(.schedule, "Schedule", icon: "tablecells", badge: places)
            place(.workflow, "Workflow", icon: "point.3.connected.trianglepath.dotted", badge: nil)
            place(.usage, "Usage", icon: "chart.bar", badge: nil)

            let groups = grouped(issues)
            group(.needsAPerson, groups.first { $0.group == .needsAPerson }?.issues ?? [])
            ForEach(groups.filter { $0.group != .needsAPerson }, id: \.group) { section in
                group(section.group, section.issues)
            }
        }
        .padding(.horizontal, 10).padding(.vertical, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Theme.sidebar)
    }

    private func place(_ target: Screen, _ title: String, icon: String, badge: String?) -> some View {
        let on = screen == target
        return Button { go(target) } label: {
            HStack(spacing: 8) {
                Image(systemName: icon).font(.system(size: 13)).foregroundStyle(on ? Color.white : Theme.accent).frame(width: 16)
                Text(title).font(.system(size: 13)).foregroundStyle(on ? Color.white : Theme.text)
                Spacer(minLength: 0)
                if let badge {
                    Text(badge).font(.system(size: 12).monospacedDigit()).foregroundStyle(on ? Color.white : Theme.secondary)
                }
            }
            .padding(.horizontal, 8).padding(.vertical, 5)
            .background(on ? Theme.accent : Color.clear, in: RoundedRectangle(cornerRadius: 6))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
    }

    @ViewBuilder
    private func group(_ group: IssueGroup, _ members: [Issue]) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(group.rawValue)
            Spacer()
            Text("\(members.count)").monospacedDigit()
        }
        .font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.secondary)
        .padding(.horizontal, 8).padding(.top, 14).padding(.bottom, 4)
        if members.isEmpty {
            Text("Nothing needs you.").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                .padding(.horizontal, 8).padding(.bottom, 6)
        }
        ForEach(members) { issue in
            Button { select(issue.id) } label: {
                IssueRowView(issue: issue, selected: screen == .issue && issue.id == selected, showRepo: several, parked: parked.contains(issue.id))
            }
            .buttonStyle(.plain)
        }
    }
}

struct IssueRowView: View {
    var issue: Issue
    var selected: Bool
    /// Says which repository the issue is in. Two projects both have a #4.
    var showRepo = false
    var parked = false

    var body: some View {
        let kind = mark(of: issue, parked: parked)
        HStack(alignment: .top, spacing: 9) {
            MarkView(mark: kind, selected: selected).padding(.top, 1)
            VStack(alignment: .leading, spacing: 0) {
                Text("#\(issue.number) \(issue.title)").font(.system(size: 13, weight: .medium))
                    .foregroundStyle(selected ? Color.white : Theme.text).lineLimit(2)
                Text(stateCaption(issue, showRepo: showRepo)).font(.system(size: 12))
                    .foregroundStyle(selected ? Color(hex: 0xDCE9FA) : (kind == .pullRequest ? Theme.purple : Theme.secondary))
                if !issue.waitsOn.isEmpty, issue.state != "pr-open" {
                    Text(issue.waitsOn).font(.system(size: 12)).lineLimit(2)
                        .foregroundStyle(selected ? Color.white : (kind == .needs ? Theme.red : Theme.amber))
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8).padding(.vertical, 6)
        .background(selected ? Theme.accent : Color.clear, in: RoundedRectangle(cornerRadius: 6))
        .contentShape(Rectangle())
    }
}

/// One issue: where it stands in the workflow, what a person should look at,
/// each step with its time, and what the running agent or check prints.
struct IssueDetailPane: View {
    var issue: Issue
    var detail: IssueDetail?
    var log: LogTail?
    /// The workflow's step names in order, when the engine knows them.
    var order: [String]?
    var parked = false
    /// The "to look at" items a person has ticked, by `issue id|item`.
    var looked: Set<String> = []
    var toggleLooked: (String) -> Void = { _ in }
    var retry: () -> Void = {}
    var drop: () -> Void = {}
    @State private var outputOpen: Bool?

    var body: some View {
        let steps = detail?.steps ?? []
        VStack(alignment: .leading, spacing: 20) {
            header(steps: steps.count)

            if !steps.isEmpty {
                let active = issue.state == "running" || issue.state == "waiting" || issue.state == "needs-a-person"
                Boxed {
                    Flow {
                        let chips = stageChips(steps: steps, order: order, current: active ? issue.stage : nil, stuck: issue.state == "needs-a-person")
                        ForEach(Array(chips.enumerated()), id: \.offset) { index, chip in
                            HStack(spacing: 4) {
                                chipView(chip)
                                if index < chips.count - 1 {
                                    Image(systemName: "chevron.right").font(.system(size: 8, weight: .bold)).foregroundStyle(Theme.idle)
                                }
                            }
                        }
                    }
                    .padding(.horizontal, 14).padding(.vertical, 12)
                }
            }

            if issue.state == "needs-a-person" {
                Boxed(fill: Theme.redFill, line: Theme.redLine) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Needs you").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.redText)
                        Text(detail?.failure ?? issue.waitsOn).font(.system(size: 13)).foregroundStyle(Theme.text).lineLimit(8)
                        if let judgment = issue.judgment {
                            Text("Judgment: \(judgment)").font(.system(size: 13)).foregroundStyle(Theme.tertiary).lineLimit(10)
                        }
                        HStack(spacing: 8) {
                            OutlineButton(title: "Start over", action: retry)
                            OutlineButton(title: "Take off the queue", action: drop)
                        }
                    }
                    .padding(.horizontal, 18).padding(.vertical, 16)
                }
            }

            if let summary = detail?.summary {
                VStack(alignment: .leading, spacing: 10) {
                    SectionTitle(title: "Summary", size: 15)
                    Text(summary.whatChanged).font(.system(size: 13)).foregroundStyle(Theme.text).fixedSize(horizontal: false, vertical: true)
                    Text(summary.howItWent).font(.system(size: 13)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
                }
                .frame(maxWidth: 620, alignment: .leading)
                if !summary.forAHuman.isEmpty {
                    toLookAt(summary.forAHuman)
                }
            }

            if !steps.isEmpty {
                stepList(steps)
            }

            if let log, !log.text.isEmpty {
                output(log)
            }
        }
        .frame(maxWidth: 960, alignment: .leading)
    }

    private func header(steps: Int) -> some View {
        let kind = mark(of: issue, parked: parked)
        let look: (fill: Color, ink: Color) = {
            switch kind {
            case .done: return (Theme.greenFill, Theme.greenText)
            case .running: return (Theme.blueFill, Theme.blueText)
            case .waiting, .parked: return (Theme.amberFill, Theme.amberDark)
            case .needs: return (Theme.redFill, Theme.redText)
            case .pullRequest: return (Theme.purpleFill, Theme.purple)
            default: return (Theme.hairline, Theme.tertiary)
            }
        }()
        return VStack(alignment: .leading, spacing: 6) {
            Text("\(issue.repo) · #\(issue.number)").font(.system(size: 13)).foregroundStyle(Theme.secondary)
            Text(issue.title).font(.system(size: 24, weight: .semibold)).foregroundStyle(Theme.text)
            HStack(spacing: 8) {
                Pill(text: (kind == .done ? "✓ " : "") + stateLabel(issue), fill: look.fill, ink: look.ink, bold: true)
                if issue.pass > 1 { Pill(text: "Pass \(issue.pass)") }
                if steps > 0 { Pill(text: "\(steps) step\(steps == 1 ? "" : "s")") }
                Spacer(minLength: 0)
                if let url = issue.prUrl, let link = URL(string: url) {
                    OutlineButton(title: "Open pull request #\(link.lastPathComponent) ↗") { NSWorkspace.shared.open(link) }
                }
            }
            .padding(.top, 4)
            if !issue.waitsOn.isEmpty && issue.state != "needs-a-person" {
                Text("Waiting on: \(issue.waitsOn)").font(.system(size: 13)).foregroundStyle(Theme.amber)
            }
        }
    }

    private func chipView(_ chip: StageChip) -> some View {
        let look: (fill: Color, ink: Color, line: Color?) = {
            switch chip.state {
            case "ok": return (Theme.greenFill, Theme.greenText, nil)
            case "skip": return (Theme.hairline, Theme.secondary, nil)
            case "fail": return (Theme.redFill, Theme.redText, nil)
            case "current": return (Theme.blueFill, Theme.blueText, nil)
            default: return (.white, Theme.secondary, Theme.border)
            }
        }()
        return HStack(spacing: 4) {
            if !chip.mark.isEmpty { Text(chip.mark) }
            Text(chip.name)
            if !chip.extra.isEmpty { Text(chip.extra.trimmingCharacters(in: .whitespaces)).fontWeight(.regular).foregroundStyle(Theme.secondary) }
        }
        .font(.system(size: 13, weight: .semibold)).foregroundStyle(look.ink)
        .padding(.horizontal, 9).frame(height: 26)
        .background(look.fill, in: RoundedRectangle(cornerRadius: 6))
        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(look.line ?? .clear, lineWidth: 1))
    }

    private func toLookAt(_ items: [String]) -> some View {
        Boxed(fill: Theme.amberCard, line: Theme.amberCardLine) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    Image(systemName: "eye").font(.system(size: 13)).foregroundStyle(Theme.amber)
                    Text("To look at").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.text)
                    Text("\(items.count) item\(items.count == 1 ? "" : "s")").font(.system(size: 15)).foregroundStyle(Theme.amberDark)
                }
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    let key = "\(issue.id)|\(item)"
                    let done = looked.contains(key)
                    Button { toggleLooked(key) } label: {
                    HStack(alignment: .top, spacing: 10) {
                        ZStack {
                            RoundedRectangle(cornerRadius: 3).fill(done ? Theme.accent : Color.white)
                            RoundedRectangle(cornerRadius: 3).strokeBorder(done ? Theme.accent : Theme.idle, lineWidth: 1)
                            if done { Text("✓").font(.system(size: 10, weight: .bold)).foregroundStyle(.white) }
                        }
                        .frame(width: 14, height: 14).padding(.top, 2)
                        Text(item).font(.system(size: 13)).foregroundStyle(done ? Theme.secondary : Theme.text)
                            .fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel((done ? "Looked at: " : "To look at: ") + item)
                }
            }
            .padding(.horizontal, 18).padding(.vertical, 16)
        }
    }

    private func stepList(_ steps: [StepRow]) -> some View {
        let longest = steps.map(\.seconds).max() ?? 0
        return VStack(alignment: .leading, spacing: 10) {
            SectionTitle(title: "Steps", note: "bars show time, longest \(duration(longest))", size: 15)
            ForEach(passGroups(steps)) { group in
                Boxed {
                    VStack(spacing: 0) {
                        HStack {
                            Text("Pass \(group.pass)").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                            Spacer()
                            Text(group.note).font(.system(size: 13)).foregroundStyle(Theme.secondary)
                        }
                        .padding(.horizontal, 14).padding(.vertical, 8).background(Theme.band)
                        ForEach(Array(group.steps.enumerated()), id: \.offset) { _, step in
                            Rectangle().fill(Theme.hairline).frame(height: 1)
                            stepRow(step, longest: longest)
                        }
                    }
                }
            }
        }
    }

    private func stepRow(_ step: StepRow, longest: Double) -> some View {
        let bad = failingVerdicts.contains(step.verdict)
        let warn = step.verdict == "skipped" || step.verdict == "rate-limited"
        let ink = bad ? Theme.red : (warn ? Theme.amber : Theme.green)
        let who = Theme.vendor(step.vendor)
        return HStack(spacing: 12) {
            Text(bad ? "✕" : (warn ? "–" : "✓")).font(.system(size: 12, weight: .bold)).foregroundStyle(ink).frame(width: 20, alignment: .leading)
            Text(step.step).font(.system(size: 13)).foregroundStyle(Theme.text).frame(width: 150, alignment: .leading)
            Text(stepWho(step)).font(.system(size: 12, weight: .medium)).foregroundStyle(who.text).lineLimit(1).frame(width: 150, alignment: .leading)
            Text(step.verdict).font(.system(size: 13, weight: bad ? .semibold : .regular))
                .foregroundStyle(bad ? Theme.red : (warn ? Theme.amber : Theme.text)).frame(width: 100, alignment: .leading)
            MeterBar(share: longest > 0 ? max(0.015, step.seconds / longest) : 0, color: who.bar)
            Text(duration(step.seconds)).font(.system(size: 13).monospacedDigit()).foregroundStyle(Theme.secondary).frame(width: 84, alignment: .trailing)
        }
        .padding(.horizontal, 14).padding(.vertical, 5)
    }

    private func output(_ log: LogTail) -> some View {
        // Open while the issue is at work, closed once it has ended.
        let open = outputOpen ?? (issue.state == "running" || issue.state == "waiting" || issue.state == "needs-a-person")
        return Boxed {
            VStack(alignment: .leading, spacing: 10) {
                Button { outputOpen = !open } label: {
                HStack(spacing: 8) {
                    Image(systemName: open ? "chevron.down" : "chevron.right").font(.system(size: 9, weight: .bold)).foregroundStyle(Theme.secondary)
                    Text("Latest output").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                    if let file = log.file {
                        Text(file).font(.system(size: 12, design: .monospaced)).foregroundStyle(Theme.secondary)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(open ? "Hide the latest output" : "Show the latest output")
                if open {
                    Text(logExcerpt(log.text))
                        .font(.system(size: 12, design: .monospaced)).foregroundStyle(Color(hex: 0xE8E8EA))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(12)
                        .background(Color(hex: 0x1E1E20), in: RoundedRectangle(cornerRadius: 8))
                        .textSelection(.enabled)
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
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
