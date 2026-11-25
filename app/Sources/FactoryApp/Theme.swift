import SwiftUI

// The look of the app, taken from the redesign canvas: its colors, and the
// small pieces every screen shares. The design is drawn for a light window,
// so the app is light whatever the system is set to.

extension Color {
    init(hex: UInt32) {
        self.init(.sRGB, red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255, blue: Double(hex & 0xFF) / 255, opacity: 1)
    }
}

enum Theme {
    static let text = Color(hex: 0x1D1D1F)
    static let secondary = Color(hex: 0x6E6E73)
    static let tertiary = Color(hex: 0x48484A)
    static let border = Color(hex: 0xE5E5E8)
    static let hairline = Color(hex: 0xF0F0F2)
    static let control = Color(hex: 0xD2D2D6)
    static let band = Color(hex: 0xF6F6F7)
    static let sidebar = Color(hex: 0xF2F2F4)
    static let field = Color(hex: 0xE4E4E7)
    static let accent = Color(hex: 0x0062CC)

    static let green = Color(hex: 0x1E8E3E)
    static let greenText = Color(hex: 0x1A6B32)
    static let greenFill = Color(hex: 0xE3F4E7)
    static let amber = Color(hex: 0xA65A00)
    static let amberDark = Color(hex: 0x8A5300)
    static let amberLine = Color(hex: 0xC77700)
    static let amberFill = Color(hex: 0xFFF1DB)
    static let amberCard = Color(hex: 0xFFF8EB)
    static let amberCardLine = Color(hex: 0xF2DDB3)
    static let red = Color(hex: 0xC8251E)
    static let redText = Color(hex: 0xA51F18)
    static let redFill = Color(hex: 0xFDECEA)
    static let redLine = Color(hex: 0xF3C9C5)
    static let purple = Color(hex: 0x6E2FB0)
    static let purpleLine = Color(hex: 0x8A4BD0)
    static let purpleFill = Color(hex: 0xF1E8FB)
    static let blueText = Color(hex: 0x1F4E8C)
    static let blueFill = Color(hex: 0xE4EEFB)
    static let blueCard = Color(hex: 0xF1F6FD)
    static let blueLine = Color(hex: 0x3B7DD8)
    static let free = Color(hex: 0xC7C7CC)
    static let idle = Color(hex: 0xAEAEB2)

    /// The fill, the outline and the small text of a workflow node, by kind.
    static func node(_ kind: String) -> (fill: Color, line: Color, sub: Color) {
        switch kind {
        case "agent": return (Color(hex: 0xF3F7FD), blueLine, Color(hex: 0x1F5FA8))
        case "check": return (Color(hex: 0xF0F9F8), Color(hex: 0x1F8A80), Color(hex: 0x17685F))
        case "judgment": return (purpleFill, purpleLine, purple)
        default: return (band, idle, secondary)
        }
    }

    /// The color a vendor's bars and name are drawn in.
    static func vendor(_ name: String?) -> (bar: Color, text: Color) {
        switch name {
        case "claude": return (Color(hex: 0xD07A45), Color(hex: 0x9A4A1F))
        case "codex", "chatgpt": return (blueLine, Color(hex: 0x1F5FA8))
        case "glm": return (Color(hex: 0x1F8A80), Color(hex: 0x17685F))
        case .none: return (Color(hex: 0xA1A1A6), secondary)
        default: return (purpleLine, purple)
        }
    }
}

/// What the round mark in front of an issue says about it.
enum Mark: Equatable {
    case needs, running, waiting, parked, notStarted, pullRequest, done, dropped
}

/// The 18-point mark in front of an issue. `selected` is for a row drawn on
/// the accent color.
struct MarkView: View {
    var mark: Mark
    var selected = false

    var body: some View {
        let look = look
        ZStack {
            Circle().fill(selected ? Color.white : look.fill)
            if let line = look.line, !selected {
                Circle().strokeBorder(line, style: StrokeStyle(lineWidth: 1.5, dash: mark == .parked ? [3, 2] : []))
            }
            Text(look.glyph).font(.system(size: 10, weight: .bold)).foregroundStyle(selected ? Theme.accent : look.ink)
        }
        .frame(width: 18, height: 18)
    }

    private var look: (glyph: String, fill: Color, line: Color?, ink: Color) {
        switch mark {
        case .needs: return ("!", Theme.red, nil, .white)
        case .running: return ("▶", Theme.accent, nil, .white)
        case .waiting: return ("‖", Theme.amberFill, Theme.amberLine, Theme.amber)
        case .parked: return ("⏸", Theme.amberFill, Theme.amberLine, Theme.amber)
        case .notStarted: return ("", .white, Theme.idle, Theme.secondary)
        case .pullRequest: return ("↑", Theme.purpleFill, Theme.purpleLine, Theme.purple)
        case .done: return ("✓", Theme.green, nil, .white)
        case .dropped: return ("–", Theme.hairline, Theme.idle, Theme.secondary)
        }
    }
}

/// A rounded label, such as "Merged" or "Pass 2".
struct Pill: View {
    var text: String
    var fill = Theme.hairline
    var ink = Theme.tertiary
    var bold = false
    var radius: CGFloat = 12

    var body: some View {
        Text(text).font(.system(size: 13, weight: bold ? .semibold : .regular)).foregroundStyle(ink)
            .padding(.horizontal, 9).frame(height: 24)
            .background(fill, in: RoundedRectangle(cornerRadius: radius))
    }
}

/// A box with the design's hairline border.
struct Boxed<Content: View>: View {
    var fill = Color.white
    var line = Theme.border
    /// Grows to the height it is offered, so boxes in a row end level.
    var stretch = false
    @ViewBuilder var content: Content

    var body: some View {
        content
            .frame(maxWidth: .infinity, maxHeight: stretch ? .infinity : nil, alignment: .topLeading)
            .background(fill)
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(line, lineWidth: 1))
    }
}

/// A thin bar filled to a share of its width.
struct MeterBar: View {
    /// 0 to 1.
    var share: Double
    var color: Color
    var height: CGFloat = 6

    var body: some View {
        GeometryReader { space in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.hairline)
                Capsule().fill(color).frame(width: max(height, space.size.width * min(1, max(0, share))))
            }
        }
        .frame(height: height)
    }
}

/// A screen's heading: a small line above, the title, and a line below.
struct ScreenTitle: View {
    var over: String
    var title: String
    var under: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(over).font(.system(size: 13)).foregroundStyle(Theme.secondary)
            Text(title).font(.system(size: 24, weight: .semibold)).foregroundStyle(Theme.text)
            if let under {
                Text(under).font(.system(size: 13)).foregroundStyle(Theme.tertiary)
            }
        }
    }
}

/// A second-level heading, with an optional note beside it.
struct SectionTitle: View {
    var title: String
    var note: String?
    var size: CGFloat = 17

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(title).font(.system(size: size, weight: .semibold)).foregroundStyle(Theme.text)
            if let note {
                Text(note).font(.system(size: 13)).foregroundStyle(Theme.secondary)
            }
        }
    }
}

/// A button drawn as the design's outlined control. Plain shapes and a tap,
/// so it also draws to an image.
struct OutlineButton: View {
    var title: String
    var enabled = true
    var action: () -> Void

    var body: some View {
        Text(title).font(.system(size: 13)).foregroundStyle(enabled ? Theme.text : Theme.idle)
            .padding(.horizontal, 12).frame(height: 28)
            .background(Color.white, in: RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.control, lineWidth: 1))
            .contentShape(Rectangle())
            .onTapGesture { if enabled { action() } }
            .accessibilityAddTraits(.isButton)
    }
}

/// Equal columns, as many as fit at `minimum` width each, filled row by row.
/// Not lazy, so it also draws to an image.
struct Columns: Layout {
    var minimum: CGFloat
    var spacing: CGFloat = 16

    private func plan(_ subviews: Subviews, width: CGFloat) -> (count: Int, column: CGFloat, heights: [CGFloat]) {
        let count = max(1, min(subviews.count, Int((width + spacing) / (minimum + spacing))))
        let column = (width - CGFloat(count - 1) * spacing) / CGFloat(count)
        var heights: [CGFloat] = []
        for start in stride(from: 0, to: subviews.count, by: count) {
            let row = subviews[start..<min(start + count, subviews.count)]
            heights.append(row.map { $0.sizeThatFits(ProposedViewSize(width: column, height: nil)).height }.max() ?? 0)
        }
        return (count, column, heights)
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? (minimum * CGFloat(max(1, subviews.count)) + spacing * CGFloat(max(0, subviews.count - 1)))
        let plan = plan(subviews, width: width)
        return CGSize(width: width, height: plan.heights.reduce(0, +) + CGFloat(max(0, plan.heights.count - 1)) * spacing)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let plan = plan(subviews, width: bounds.width)
        var y = bounds.minY
        for (row, height) in plan.heights.enumerated() {
            for column in 0..<plan.count {
                let index = row * plan.count + column
                guard index < subviews.count else { break }
                let x = bounds.minX + CGFloat(column) * (plan.column + spacing)
                subviews[index].place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(width: plan.column, height: height))
            }
            y += height + spacing
        }
    }
}
