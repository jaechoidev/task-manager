import XCTest
@testable import FactoryApp

final class ModelTests: XCTestCase {
    /// What `GET /v1/issues` answers, taken from a real run.
    let issuesJSON = """
    [{"head_sha":null,"judgment":"escalate. The simulator is broken.","key":"tabs","number":24,"pass":2,"pr_url":null,"repo":"o/r","run_id":"20261124T203119Z","stage":"verify","state":"needs-a-person","status":"failed","title":"Tab bar","updated_at":"2026-11-24T20:33:02Z","waits_on":"3 fix rounds were used."},
     {"head_sha":"794efbad","judgment":null,"key":"filter","number":7,"pass":2,"pr_url":"https://example.invalid/pull/19","repo":"o/r","run_id":"r","stage":"merged","state":"merged","status":"done","title":"Filter","updated_at":"2026-11-24T19:37:02Z","waits_on":""},
     {"head_sha":"abc","judgment":null,"key":"stats","number":25,"pass":1,"pr_url":"https://example.invalid/pull/30","repo":"o/r","run_id":"r","stage":"github","state":"waiting","status":"running","title":"Stats","updated_at":"2026-11-24T20:40:00Z","waits_on":"GitHub's checks and review"},
     {"head_sha":"abc","judgment":null,"key":"add","number":5,"pass":1,"pr_url":"https://example.invalid/pull/14","repo":"o/r","run_id":"r","stage":"pr","state":"pr-open","status":"open","title":"Add","updated_at":"2026-11-24T18:12:31Z","waits_on":"a person to look at it"}]
    """

    func testIssuesDecodeAndGroupByWhatAPersonNeedsFirst() throws {
        let issues = try JSONDecoder.api.decode([Issue].self, from: Data(issuesJSON.utf8))
        XCTAssertEqual(issues.count, 4)
        XCTAssertEqual(issues[0].waitsOn, "3 fix rounds were used.")
        XCTAssertEqual(issues[0].path, "issues/o/r/24")
        let groups = grouped(issues)
        XCTAssertEqual(groups.map(\.group), [.needsAPerson, .active, .open, .done])
        XCTAssertEqual(groups.map { $0.issues.map(\.number) }, [[24], [25], [5], [7]])
        XCTAssertEqual(stateLabel(issues[0]), "Stopped at verify")
        XCTAssertEqual(stateLabel(issues[2]), "Waiting at github")
    }

    func testDetailDecodesWithAndWithoutASummary() throws {
        let json = """
        {"repo":"o/r","number":7,"title":"Filter","state":"merged","stage":"merged","status":"done","waits_on":"","pass":2,"pr_url":"https://example.invalid/pull/19","head_sha":"794e","judgment":null,"failure":null,"key":"filter","run_id":"r","updated_at":"t",
         "summary":{"what_changed":"Adds a filter.","how_it_went":"One fix round.","for_a_human":["Look once"]},
         "steps":[{"step":"develop","vendor":"codex","attempt":1,"verdict":"replied","exit_code":0,"seconds":32.4,"pass":1},{"step":"scope","vendor":null,"attempt":1,"verdict":"pass","exit_code":null,"seconds":0.0,"pass":1}],
         "run_dir":"/tmp/x","files":["a.log"]}
        """
        let detail = try JSONDecoder.api.decode(IssueDetail.self, from: Data(json.utf8))
        XCTAssertEqual(detail.summary?.forAHuman, ["Look once"])
        XCTAssertEqual(detail.steps.map(\.step), ["develop", "scope"])
        XCTAssertNil(detail.steps[1].vendor)
        let bare = json.replacingOccurrences(of: #""summary":\{[^}]*\},"#, with: #""summary":null,"#, options: .regularExpression)
        XCTAssertNil(try JSONDecoder.api.decode(IssueDetail.self, from: Data(bare.utf8)).summary)
    }

    func testALogExcerptKeepsTheEndAndCutsLongLines() {
        let text = (1...40).map { "line \($0)" }.joined(separator: "\n") + "\n" + String(repeating: "x", count: 500) + "\n"
        let excerpt = logExcerpt(text, lines: 3, width: 20)
        XCTAssertEqual(excerpt, "line 39\nline 40\n" + String(repeating: "x", count: 20) + " ...")
        XCTAssertEqual(logExcerpt(""), "")
    }

    func testDurationsReadWell() {
        XCTAssertEqual(duration(0.4), "under 1 s")
        XCTAssertEqual(duration(32.4), "32 s")
        XCTAssertEqual(duration(184), "3 min 4 s")
    }

    func workflow() throws -> Workflow {
        let json = """
        {"branch":"dev","steps":[
          {"name":"acceptance","module":"agent/acceptance","kind":"agent","vendor":"claude","locks":[]},
          {"name":"red","module":"check/acceptance-red","kind":"check","vendor":null,"locks":["simulator"]},
          {"name":"develop","module":"agent/dev","kind":"agent","vendor":"codex","locks":[]},
          {"name":"verify","module":"check/verify","kind":"check","vendor":null,"locks":["simulator"]},
          {"name":"review","module":"agent/review","kind":"agent","vendor":"claude","locks":[]},
          {"name":"sync","module":"engine/sync","kind":"engine","vendor":null,"locks":[]},
          {"name":"pr","module":"engine/pr","kind":"engine","vendor":null,"locks":[]},
          {"name":"github","module":"engine/github","kind":"engine","vendor":null,"locks":[]},
          {"name":"merge","module":"engine/merge","kind":"engine","vendor":null,"locks":[]}],
         "on_fail":{"github":"develop","red":"acceptance","review":"develop","verify":"develop"},
         "judgment":{"name":"judgment","module":"judgment/stuck","kind":"judgment","vendor":"claude","locks":[]},
         "summary":null,
         "limits":{"fix_rounds":3,"github_fix_rounds":2,"judgments":2}}
        """
        return try JSONDecoder.api.decode(Workflow.self, from: Data(json.utf8))
    }

    func testTheGraphPutsStepsInARowAndFailuresAboveThem() throws {
        let layout = GraphLayout.make(try workflow())
        XCTAssertEqual(layout.nodes.count, 9)
        // Steps go left to right without overlapping.
        for (a, b) in zip(layout.nodes, layout.nodes.dropFirst()) {
            XCTAssertLessThan(a.frame.maxX, b.frame.minX)
            XCTAssertEqual(a.frame.minY, b.frame.minY)
        }
        // Each failure arrow points back, and a longer one travels higher.
        XCTAssertEqual(layout.backEdges.count, 4)
        XCTAssertTrue(layout.backEdges.allSatisfy { $0.to < $0.from })
        let lengths = layout.backEdges.map { $0.from - $0.to }
        XCTAssertEqual(lengths, lengths.sorted())
        XCTAssertEqual(layout.backEdges.map(\.lift), layout.backEdges.map(\.lift).sorted())
        XCTAssertEqual(Set(layout.backEdges.map(\.lift)).count, 4, "no two arrows share a height")
        // Every arrow fits above the nodes, inside the drawing.
        XCTAssertGreaterThan(layout.nodes[0].frame.minY, layout.backEdges.last!.lift)
        XCTAssertEqual(layout.size.height, layout.nodes[0].frame.maxY)
    }

    func testTheEnginesAnswersAboutADraftDecode() throws {
        let invalid = #"{"ok":false,"errors":["step 'verify' can fail and has no on_fail"],"workflow":null}"#
        let checked = try JSONDecoder.api.decode(Validation.self, from: Data(invalid.utf8))
        XCTAssertFalse(checked.ok)
        XCTAssertEqual(checked.errors.count, 1)
        XCTAssertNil(checked.workflow)
        let modules = #"[{"module":"check/verify","kind":"check","role":null,"needs":["worktree","head_sha"],"produces":["verify_log"],"locks":["simulator"],"source":"built-in","problem":null}]"#
        XCTAssertEqual(try JSONDecoder.api.decode([ModuleInfo].self, from: Data(modules.utf8))[0].locks, ["simulator"])
        let refused = #"{"ok":false,"error":"an agent module needs a role"}"#
        XCTAssertEqual(try JSONDecoder.api.decode(Proposal.self, from: Data(refused.utf8)).problems, ["an agent module needs a role"])
        let opened = #"{"ok":true,"url":"https://example.invalid/pull/9","files":["modules/check/lint/module.yaml"]}"#
        XCTAssertEqual(try JSONDecoder.api.decode(Proposal.self, from: Data(opened.utf8)).files?.count, 1)
    }

    func testAddingAStepPutsItInTheListAndGivesAChecksFailureAWayBack() {
        let text = "# The workflow\nworkflow:\n  - develop: { module: agent/dev, vendor: codex }\n  - verify:  { module: check/verify }\non_fail:\n  verify: develop\nlimits:\n  fix_rounds: 3\n"
        let lint = ModuleInfo(module: "check/lint", kind: "check", role: nil, needs: [], produces: [], locks: [], source: "repository", problem: nil)
        let withCheck = insertStep(into: text, module: lint, developer: "develop")
        XCTAssertEqual(withCheck, "# The workflow\nworkflow:\n  - develop: { module: agent/dev, vendor: codex }\n  - verify:  { module: check/verify }\n  - lint: { module: check/lint }\non_fail:\n  lint: develop\n  verify: develop\nlimits:\n  fix_rounds: 3\n")
        let review = ModuleInfo(module: "agent/review", kind: "agent", role: "review", needs: [], produces: [], locks: [], source: "built-in", problem: nil)
        let withAgent = insertStep(into: text, module: review, developer: "develop")
        XCTAssertTrue(withAgent.contains("  - review: { module: agent/review, vendor: claude }\non_fail:\n  verify: develop"))
        // A file with no step list is left alone.
        XCTAssertEqual(insertStep(into: "limits:\n", module: lint, developer: nil), "limits:\n")
    }

    func testAnArrowToAStepThatIsNotThereIsLeftOut() throws {
        var workflow = try workflow()
        workflow.onFail = ["verify": "develop", "review": "nowhere", "acceptance": "develop"]
        let layout = GraphLayout.make(workflow)
        // Only an arrow that goes back to an existing step is drawn.
        XCTAssertEqual(layout.backEdges.map { [$0.from, $0.to] }, [[3, 2]])
    }
}
