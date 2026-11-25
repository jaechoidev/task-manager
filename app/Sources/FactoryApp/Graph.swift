import CoreGraphics

/// Where each step of a workflow sits, and which arrows connect them. Plain
/// arithmetic, so it can be tested without drawing anything.
struct GraphLayout: Equatable {
    struct Node: Equatable {
        var step: WorkflowStep
        var frame: CGRect
    }

    /// An arrow from a step that failed back to the step that fixes it.
    struct BackEdge: Equatable {
        var from: Int
        var to: Int
        /// How high above the nodes the arrow travels. Longer arrows go higher.
        var lift: CGFloat
    }

    var nodes: [Node]
    var backEdges: [BackEdge]
    var size: CGSize

    static let nodeSize = CGSize(width: 104, height: 56)
    static let gap: CGFloat = 32

    static func make(_ workflow: Workflow) -> GraphLayout {
        let names = workflow.steps.map(\.name)
        var edges: [BackEdge] = []
        for (from, target) in workflow.onFail.sorted(by: { $0.key < $1.key }) {
            guard let a = names.firstIndex(of: from), let b = names.firstIndex(of: target), b < a else { continue }
            edges.append(BackEdge(from: a, to: b, lift: 0))
        }
        // Shorter arrows sit lower, so they don't cross longer ones.
        edges.sort { ($0.from - $0.to, $0.from) < ($1.from - $1.to, $1.from) }
        for index in edges.indices {
            edges[index].lift = 16 + CGFloat(index) * 13
        }
        let top = (edges.last?.lift ?? 0) + 14
        let nodes = workflow.steps.enumerated().map { index, step in
            Node(step: step, frame: CGRect(x: CGFloat(index) * (nodeSize.width + gap), y: top, width: nodeSize.width, height: nodeSize.height))
        }
        let width = max(0, CGFloat(nodes.count) * (nodeSize.width + gap) - gap)
        return GraphLayout(nodes: nodes, backEdges: edges, size: CGSize(width: width, height: top + nodeSize.height))
    }
}
