import SwiftUI

/// A workflow's steps as a row of boxes, with a dashed red line from each
/// step that can fail back to the step it returns to. `refused` names steps
/// the engine would not accept, with a few words on why.
struct WorkflowGraph: View {
    var workflow: Workflow
    var refused: [String: String] = [:]

    var body: some View {
        let layout = GraphLayout.make(workflow)
        ZStack(alignment: .topLeading) {
            Canvas { context, _ in
                for index in layout.nodes.indices.dropLast() {
                    let a = layout.nodes[index].frame, b = layout.nodes[index + 1].frame
                    var path = Path()
                    path.move(to: CGPoint(x: a.maxX, y: a.midY))
                    path.addLine(to: CGPoint(x: b.minX - 1, y: b.midY))
                    context.stroke(path, with: .color(Theme.idle), lineWidth: 1.4)
                    context.fill(arrowHead(at: CGPoint(x: b.minX, y: b.midY), down: false), with: .color(Theme.idle))
                }
                // A failure goes back to the step that fixes it.
                for edge in layout.backEdges {
                    let from = layout.nodes[edge.from].frame, to = layout.nodes[edge.to].frame
                    let y = from.minY - edge.lift
                    var path = Path()
                    path.move(to: CGPoint(x: from.midX, y: from.minY))
                    path.addLine(to: CGPoint(x: from.midX, y: y))
                    path.addLine(to: CGPoint(x: to.midX + 10, y: y))
                    path.addLine(to: CGPoint(x: to.midX + 10, y: to.minY - 1))
                    context.stroke(path, with: .color(Theme.red), style: StrokeStyle(lineWidth: 1.4, dash: [5, 3]))
                    context.fill(arrowHead(at: CGPoint(x: to.midX + 10, y: to.minY), down: true), with: .color(Theme.red))
                }
            }
            .frame(width: layout.size.width, height: layout.size.height)
            ForEach(Array(layout.nodes.enumerated()), id: \.offset) { _, node in
                let why = refused[node.step.name]
                let look = Theme.node(node.step.kind)
                VStack(spacing: 1) {
                    Text(node.step.name).font(.system(size: 13.5, weight: .bold)).foregroundStyle(Theme.text).lineLimit(1)
                    Text(why ?? node.step.vendor ?? node.step.kind).font(.system(size: 11.5))
                        .foregroundStyle(why == nil ? look.sub : Theme.redText).lineLimit(1).minimumScaleFactor(0.75)
                }
                .padding(6)
                .frame(width: node.frame.width, height: node.frame.height)
                .background(why == nil ? look.fill : Theme.redFill, in: RoundedRectangle(cornerRadius: 9))
                .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(why == nil ? look.line : Theme.red, style: StrokeStyle(lineWidth: 1.5, dash: why == nil ? [] : [4, 3])))
                .offset(x: node.frame.minX, y: node.frame.minY)
            }
        }
        .frame(width: layout.size.width, height: layout.size.height, alignment: .topLeading)
    }

    /// A small triangle with its tip at `tip`, pointing right or down.
    private func arrowHead(at tip: CGPoint, down: Bool) -> Path {
        var path = Path()
        path.move(to: tip)
        if down {
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

/// What the colors of a graph mean.
struct GraphLegend: View {
    var withRefused = false

    var body: some View {
        Flow(spacing: 16) {
            swatch("agent", "Agent, an LLM call")
            swatch("check", "Check, a command")
            swatch("engine", "Engine")
            HStack(spacing: 6) {
                Rectangle().fill(Theme.red).frame(width: 16, height: 2)
                Text("Where a failure goes back")
            }
            if withRefused {
                HStack(spacing: 6) {
                    RoundedRectangle(cornerRadius: 3).fill(Theme.redFill)
                        .overlay(RoundedRectangle(cornerRadius: 3).strokeBorder(Theme.red, style: StrokeStyle(lineWidth: 1.5, dash: [3, 2])))
                        .frame(width: 12, height: 12)
                    Text("Refused by the engine").foregroundStyle(Theme.redText)
                }
            }
        }
        .font(.system(size: 13)).foregroundStyle(Theme.tertiary)
    }

    private func swatch(_ kind: String, _ text: String) -> some View {
        let look = Theme.node(kind)
        return HStack(spacing: 6) {
            RoundedRectangle(cornerRadius: 3).fill(look.fill)
                .overlay(RoundedRectangle(cornerRadius: 3).strokeBorder(look.line, lineWidth: 1.5))
                .frame(width: 12, height: 12)
            Text(text)
        }
    }
}

/// A repository's workflow: its graph, and the limits it runs under.
struct WorkflowPane: View {
    var workflow: Workflow
    var repo: String?
    /// Opens the editor. Nil leaves the button out.
    var edit: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 28) {
            HStack(alignment: .bottom) {
                ScreenTitle(over: "factory.yaml on \(workflow.branch)" + (repo.map { " · \($0)" } ?? ""), title: "Workflow")
                Spacer(minLength: 0)
                if let edit {
                    OutlineButton(title: "Edit workflow…", action: edit)
                }
            }
            GraphLegend()
            VStack(alignment: .leading, spacing: 8) {
                SidewaysScroll { WorkflowGraph(workflow: workflow).padding(.vertical, 4) }
                Text("Red lines: a failed step goes back to the step it points at, and every step after that runs again.")
                    .font(.system(size: 13)).foregroundStyle(Theme.secondary)
            }
            Columns(minimum: 200, spacing: 12) {
                number("\(workflow.limits.fixRounds)", "fix rounds before the pull request", "5 at most")
                number("\(workflow.limits.githubFixRounds)", "fix rounds on GitHub", "failures found on GitHub")
                number("\(workflow.limits.judgments)", "new attempts a judgment may start", "3 at most")
                Boxed(stretch: true) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(workflow.judgment?.module ?? "nobody").font(.system(size: 15, weight: .semibold)).foregroundStyle(Theme.text).padding(.top, 6)
                        Text("decides when an issue is stuck").font(.system(size: 13)).foregroundStyle(Theme.tertiary)
                        Text(workflow.judgment.map { "on \($0.vendor ?? "?")" } ?? "a stuck issue goes to a person").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                    }
                    .padding(.horizontal, 16).padding(.vertical, 14)
                }
            }
            .frame(maxWidth: 920)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func number(_ value: String, _ what: String, _ note: String) -> some View {
        Boxed(stretch: true) {
            VStack(alignment: .leading, spacing: 2) {
                Text(value).font(.system(size: 26, weight: .semibold).monospacedDigit()).foregroundStyle(Theme.text)
                Text(what).font(.system(size: 13)).foregroundStyle(Theme.tertiary)
                Text(note).font(.system(size: 12)).foregroundStyle(Theme.secondary)
            }
            .padding(.horizontal, 16).padding(.vertical, 14)
        }
    }
}

/// What each plan has left and what was spent: a card per provider's plan
/// with a button that reads them again, then calls and tokens per vendor.
/// An engine that does not count tokens gets the calls and the limits only.
struct UsagePane: View {
    var usage: [UsageRow]
    /// Where the plans' readings come from. Nil when the engine has none.
    var schedule: Schedule?
    /// Asks every provider for its plan again. Nil leaves the button out.
    var refreshQuota: (() -> Void)?
    /// Set while the providers are being asked.
    var refreshing = false

    var body: some View {
        let most = usage.compactMap(\.tokens).max() ?? 0
        let counted = most > 0
        let missing = usage.map { $0.calls - ($0.tokenCalls ?? $0.calls) }.reduce(0, +)
        VStack(alignment: .leading, spacing: 28) {
            ScreenTitle(over: "each provider's plan, and what every issue on this machine used", title: "Usage")
            if let schedule {
                VStack(alignment: .leading, spacing: 12) {
                    HStack(spacing: 10) {
                        SectionTitle(title: "Quota", note: refreshing ? "asking each provider..." : nil)
                        if let refreshQuota, !refreshing {
                            Button(action: refreshQuota) {
                                Image(systemName: "arrow.clockwise").font(.system(size: 12, weight: .medium)).foregroundStyle(Theme.tertiary)
                                    .frame(width: 26, height: 24)
                                    .background(Color.white, in: RoundedRectangle(cornerRadius: 6))
                                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.control, lineWidth: 1))
                                    .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .help("Reads every plan again. Z.ai is asked directly. Claude and ChatGPT each get one small call, because they only say it in a reply.")
                            .accessibilityLabel("Refresh quota")
                        }
                    }
                    if schedule.out.isEmpty && schedule.quota.isEmpty {
                        Text("No plan has been read yet.").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                    } else {
                        QuotaCards(out: schedule.out, readings: schedule.quota)
                    }
                }
                .frame(maxWidth: 1080, alignment: .leading)
            }
            VStack(alignment: .leading, spacing: 10) {
                SectionTitle(title: "Usage by vendor")
                if usage.isEmpty {
                    Text("No agent has been called yet.").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                } else {
                    Boxed {
                        VStack(spacing: 0) {
                            row("Vendor", "Calls", nil, counted ? "Tokens" : "", "", "Usage limits hit", header: true, counted: counted)
                            ForEach(usage) { entry in
                                Rectangle().fill(Theme.hairline).frame(height: 1)
                                row(
                                    entry.vendor, "\(entry.calls)",
                                    entry.tokens.map { (Double($0) / Double(most), Theme.vendor(entry.vendor).bar) },
                                    entry.tokens.map(tokenCount) ?? "", tokenDetail(entry), "\(entry.rateLimited)",
                                    header: false, counted: counted, warn: entry.rateLimited > 0
                                )
                            }
                        }
                    }
                    if counted {
                        Text("Tokens are what each program counted for its own calls: everything sent to the model, cached or not, and everything it wrote." + (missing > 0 ? " \(missing) call\(missing == 1 ? " has" : "s have") no count." : ""))
                            .font(.system(size: 12)).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .frame(maxWidth: 860, alignment: .leading)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func row(_ vendor: String, _ calls: String, _ bar: (Double, Color)?, _ tokens: String, _ detail: String, _ limits: String, header: Bool, counted: Bool, warn: Bool = false) -> some View {
        HStack(spacing: 14) {
            Text(vendor).fontWeight(header ? .semibold : .medium).frame(width: 110, alignment: .leading)
            Text(calls).frame(width: 60, alignment: .trailing)
            if counted {
                Text(tokens).frame(width: 64, alignment: .trailing)
                if let bar {
                    MeterBar(share: bar.0, color: bar.1, height: 8).frame(minWidth: 80)
                } else {
                    Spacer(minLength: 0)
                }
                Text(detail).font(.system(size: 12).monospacedDigit()).foregroundStyle(Theme.secondary).lineLimit(1).frame(width: 240, alignment: .leading)
            } else {
                Spacer(minLength: 0)
            }
            Text(limits).foregroundStyle(warn ? Theme.amber : (header ? Theme.secondary : Theme.text)).frame(width: 120, alignment: .trailing)
        }
        .font(.system(size: header ? 12 : 13, weight: header ? .semibold : .regular).monospacedDigit())
        .foregroundStyle(header ? Theme.secondary : Theme.text)
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(header ? Theme.band : Color.clear)
    }
}

/// A card per provider: who is set aside after a usage limit, and how much
/// of each plan is used, with a bar per window.
struct QuotaCards: View {
    var out: [OutOfQuota]
    var readings: [QuotaReading]

    var body: some View {
        Columns(minimum: 300, spacing: 12) {
            ForEach(out) { entry in
                setAside(entry)
            }
            ForEach(readings) { reading in
                readingCard(reading)
            }
        }
    }

    private func setAside(_ entry: OutOfQuota) -> some View {
        Boxed(fill: Theme.amberCard, line: Theme.amberCardLine) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Image(systemName: "clock").font(.system(size: 13)).foregroundStyle(Theme.amber)
                    Text("\(entry.who) is set aside").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                }
                Text("until \(clockTime(entry.until))" + (entry.hits > 1 ? ", after \(entry.hits) limits in a row" : ""))
                    .font(.system(size: 13)).foregroundStyle(Theme.amberDark)
                Text("\"\(entry.reason)\"").font(.system(size: 12)).foregroundStyle(Theme.secondary).lineLimit(2)
            }
            .padding(.horizontal, 16).padding(.vertical, 14)
        }
    }

    private func readingCard(_ reading: QuotaReading) -> some View {
        let out = reading.outUntil
        return Boxed(fill: out == nil ? .white : Theme.amberCard, line: out == nil ? Theme.border : Theme.amberCardLine) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline) {
                    Text(reading.provider).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                    Spacer()
                    Text("read at \(clockTime(reading.at))").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                }
                if let out {
                    Text("used up, set aside until \(clockTime(out))").font(.system(size: 13)).foregroundStyle(Theme.amberDark)
                }
                ForEach(Array(reading.windows.enumerated()), id: \.offset) { _, window in
                    let line = quotaLine(window)
                    if let share = line.share {
                        HStack {
                            Text(line.name)
                            Spacer()
                            Text(line.used).monospacedDigit()
                        }
                        .font(.system(size: 13)).foregroundStyle(Theme.tertiary)
                        MeterBar(share: share, color: share >= 1 ? Theme.red : (share >= 0.8 ? Theme.amberLine : Theme.green))
                        if !line.detail.isEmpty {
                            Text(line.detail).font(.system(size: 12)).foregroundStyle(Theme.secondary)
                        }
                    } else {
                        Text(window).font(.system(size: 12)).foregroundStyle(Theme.secondary)
                    }
                }
            }
            .padding(.horizontal, 16).padding(.vertical, 14)
        }
    }
}
