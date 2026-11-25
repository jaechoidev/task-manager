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

/// Calls and time per vendor, with a bar for the time.
struct UsagePane: View {
    var usage: [UsageRow]

    var body: some View {
        let longest = usage.map(\.seconds).max() ?? 0
        VStack(alignment: .leading, spacing: 20) {
            ScreenTitle(over: "every issue this machine has run", title: "Usage")
            VStack(alignment: .leading, spacing: 10) {
                SectionTitle(title: "Usage by vendor")
                if usage.isEmpty {
                    Text("No agent has been called yet.").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                } else {
                    Boxed {
                        VStack(spacing: 0) {
                            row("Vendor", "Calls", nil, "Time", "Usage limits hit", header: true)
                            ForEach(usage) { entry in
                                Rectangle().fill(Theme.hairline).frame(height: 1)
                                row(entry.vendor, "\(entry.calls)", (longest > 0 ? entry.seconds / longest : 0, Theme.vendor(entry.vendor).bar), duration(entry.seconds), "\(entry.rateLimited)", header: false, warn: entry.rateLimited > 0)
                            }
                        }
                    }
                }
            }
            .frame(maxWidth: 760, alignment: .leading)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func row(_ vendor: String, _ calls: String, _ bar: (Double, Color)?, _ time: String, _ limits: String, header: Bool, warn: Bool = false) -> some View {
        HStack(spacing: 14) {
            Text(vendor).fontWeight(header ? .semibold : .medium).frame(width: 110, alignment: .leading)
            Text(calls).frame(width: 60, alignment: .trailing)
            if let bar {
                MeterBar(share: bar.0, color: bar.1, height: 8)
            } else {
                Spacer(minLength: 0)
            }
            Text(time).frame(width: 96, alignment: .leading)
            Text(limits).foregroundStyle(warn ? Theme.amber : (header ? Theme.secondary : Theme.text)).frame(width: 120, alignment: .trailing)
        }
        .font(.system(size: header ? 12 : 13, weight: header ? .semibold : .regular).monospacedDigit())
        .foregroundStyle(header ? Theme.secondary : Theme.text)
        .padding(.horizontal, 14).padding(.vertical, 8)
        .background(header ? Theme.band : Color.clear)
    }
}
