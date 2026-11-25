import SwiftUI

/// What runs where: each scheduler's places and who has them, what waits and
/// what for, each pool's slots, and who is set aside for a usage limit.
struct SchedulePane: View {
    var schedule: Schedule
    /// Only this repository's part of each scheduler, when one is picked.
    var project: String?
    /// Opens an issue, by its id.
    var open: (String) -> Void = { _ in }

    var body: some View {
        VStack(alignment: .leading, spacing: 28) {
            if schedule.schedulers.isEmpty {
                ScreenTitle(over: "factory schedule", title: "Schedule", under: "No scheduler is running. `factory schedule` starts one over the registered projects.")
            }
            ForEach(schedule.schedulers) { board in
                let projects = board.projects.filter { project == nil || $0.repo == project }
                HStack(alignment: .bottom, spacing: 16) {
                    ScreenTitle(
                        over: "factory schedule · \(board.projects.count) project\(board.projects.count == 1 ? "" : "s")",
                        title: "Schedule",
                        under: howLine(board)
                    )
                    Spacer(minLength: 0)
                    HStack(spacing: 10) {
                        HStack(spacing: 4) {
                            ForEach(0..<max(board.places, 1), id: \.self) { place in
                                RoundedRectangle(cornerRadius: 6)
                                    .fill(place < board.inUse ? Theme.accent : Color.white)
                                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(place < board.inUse ? Theme.accent : Theme.free, style: StrokeStyle(lineWidth: 1.5, dash: place < board.inUse ? [] : [4, 3])))
                                    .frame(width: 28, height: 28)
                            }
                        }
                        VStack(alignment: .leading, spacing: 0) {
                            Text("\(board.inUse) of \(board.places) place\(board.places == 1 ? "" : "s")").font(.system(size: 17, weight: .semibold).monospacedDigit()).foregroundStyle(Theme.text)
                            Text("in use").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                        }
                    }
                }
                if let reason = board.heldBack {
                    Text("Starting nothing further: \(reason)").font(.system(size: 13)).foregroundStyle(Theme.amber)
                }
                VStack(alignment: .leading, spacing: 12) {
                    SectionTitle(title: "Places")
                    Columns(minimum: 340) {
                        ForEach(projects) { project in
                            projectCard(project)
                        }
                    }
                }
            }

            if !schedule.pools.isEmpty {
                VStack(alignment: .leading, spacing: 12) {
                    SectionTitle(title: "Pools", note: "shared by every project on this machine")
                    Boxed {
                        VStack(spacing: 0) {
                            ForEach(Array(schedule.pools.enumerated()), id: \.offset) { index, pool in
                                if index > 0 { Rectangle().fill(Theme.hairline).frame(height: 1) }
                                poolRow(pool)
                            }
                        }
                    }
                }
            }

            // The plans themselves are on the Usage screen. Here only what
            // explains a wait: who is set aside, and until when.
            let usedUp = schedule.quota.filter { $0.outUntil != nil }
            if !schedule.out.isEmpty || !usedUp.isEmpty {
                VStack(alignment: .leading, spacing: 12) {
                    SectionTitle(title: "Set aside", note: "every plan is on the Usage screen")
                    QuotaCards(out: schedule.out, readings: usedUp)
                }
            }
        }
        .frame(maxWidth: 1080, alignment: .leading)
    }

    private func howLine(_ board: SchedulerBoard) -> String {
        let how = board.projects.count < 2 ? "One project" : (board.inOrder ? "One project after the other" : "Projects side by side")
        return how + (board.watch ? " · watching for new work" : "")
    }

    private func projectCard(_ project: ProjectBoard) -> some View {
        let limit = project.maxParallel.map { ", at most \($0) at once" } ?? ""
        let ended = [("Merged", project.merged), ("Pull request open", project.opened), ("Blocked", project.failed)]
            .filter { !$0.1.isEmpty }
            .map { "\($0.0): " + $0.1.map { "#\($0)" }.joined(separator: ", ") }
        return Boxed(stretch: true) {
            VStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(project.repo).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                    Text("issues labelled '\(project.label)'\(limit)").font(.system(size: 12)).foregroundStyle(Theme.secondary)
                }
                .padding(.horizontal, 14).padding(.vertical, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Theme.band)
                Rectangle().fill(Theme.border).frame(height: 1)

                if project.running.isEmpty && project.waiting.isEmpty && project.parked.isEmpty {
                    Text("Nothing is running or waiting.").font(.system(size: 13)).foregroundStyle(Theme.secondary)
                        .padding(.horizontal, 14).padding(.vertical, 12)
                }
                section("HOLDING A PLACE", project.running, in: project) { entry in
                    let waiting = entry.state == "waiting" && !(entry.waitsOn ?? "").isEmpty
                    let detail = waiting ? "waiting for \(entry.waitsOn ?? "")" : ((entry.pass ?? 1) > 1 ? "pass \(entry.pass ?? 1)" : "")
                    return (waiting ? .waiting : .running, entry.stage ?? "starting", detail, entry.since.map { duration(Double(max(0, schedule.now - $0))) } ?? "")
                }
                section("PARKED", project.parked, in: project) { entry in
                    let until = entry.until.map { "until \(clockTime($0))" } ?? "until somebody has quota"
                    return (.parked, entry.stage, until + ((entry.waitsOn ?? "").isEmpty ? "" : ": \(entry.waitsOn ?? "")"), "")
                }
                section("NOT STARTED", project.waiting, in: project) { entry in
                    (.notStarted, nil, "Waits on \(entry.waitsOn ?? "a free place")", "")
                }
                Spacer(minLength: 4)
                Rectangle().fill(Theme.hairline).frame(height: 1)
                Text(ended.isEmpty ? "Nothing has ended yet" : ended.joined(separator: ". "))
                    .font(.system(size: 12)).foregroundStyle(Theme.secondary)
                    .padding(.horizontal, 14).padding(.vertical, 10)
            }
        }
    }

    @ViewBuilder
    private func section(_ title: String, _ entries: [BoardEntry], in project: ProjectBoard, row: @escaping (BoardEntry) -> (Mark, String?, String, String)) -> some View {
        if !entries.isEmpty {
            Text("\(title) · \(entries.count)").font(.system(size: 11, weight: .semibold)).foregroundStyle(Theme.secondary)
                .padding(.horizontal, 14).padding(.top, 10).padding(.bottom, 4)
            ForEach(entries) { entry in
                let (kind, stage, detail, time) = row(entry)
                Button { open("\(project.repo)#\(entry.number)") } label: {
                HStack(alignment: .top, spacing: 10) {
                    MarkView(mark: kind).padding(.top, 1)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("#\(entry.number) \(entry.title)").font(.system(size: 13, weight: .medium)).foregroundStyle(Theme.text).lineLimit(2)
                        Flow(spacing: 6, lineSpacing: 3) {
                            if let stage {
                                Text(stage).font(.system(size: 11.5, weight: .semibold))
                                    .foregroundStyle(kind == .running ? Theme.blueText : Theme.amberDark)
                                    .padding(.horizontal, 7).padding(.vertical, 1)
                                    .background(kind == .running ? Theme.blueFill : Theme.amberFill, in: RoundedRectangle(cornerRadius: 5))
                            }
                            if !detail.isEmpty {
                                Text(detail).font(.system(size: 12)).foregroundStyle(kind == .waiting || kind == .parked ? Theme.amber : Theme.secondary)
                            }
                        }
                    }
                    Spacer(minLength: 0)
                    Text(time).font(.system(size: 13).monospacedDigit()).foregroundStyle(Theme.secondary).fixedSize()
                }
                .padding(.horizontal, 14).padding(.top, 6).padding(.bottom, 8)
                .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }

    private func poolRow(_ pool: Pool) -> some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 0) {
                Text(pool.name).font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text)
                Text("\(pool.held) of \(pool.size) held").font(.system(size: 12)).foregroundStyle(Theme.secondary)
            }
            .frame(width: 110, alignment: .leading)
            Flow(spacing: 8, lineSpacing: 8) {
                ForEach(pool.slots) { slot in
                    slotTile(slot)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
    }

    private func slotTile(_ slot: Slot) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack {
                Text("slot \(slot.slot)")
                Spacer(minLength: 8)
                if let holder = slot.holder {
                    Text(duration(Double(max(0, schedule.now - holder.since)))).monospacedDigit()
                }
            }
            .font(.system(size: 11)).foregroundStyle(Theme.secondary)
            if let holder = slot.holder {
                Text("\(shortRepo(holder.repo)) #\(holder.number)").font(.system(size: 13, weight: .semibold)).foregroundStyle(Theme.text).lineLimit(1)
                Text(holder.what).font(.system(size: 12)).foregroundStyle(Theme.tertiary).lineLimit(1)
            } else {
                Text("free").font(.system(size: 13)).foregroundStyle(Theme.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10).padding(.vertical, 7)
        .frame(width: 190, height: 58, alignment: .topLeading)
        .background(slot.holder == nil ? Color.white : Theme.blueCard, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(slot.holder == nil ? Theme.free : Theme.blueLine, style: StrokeStyle(lineWidth: 1.5, dash: slot.holder == nil ? [4, 3] : [])))
    }
}
