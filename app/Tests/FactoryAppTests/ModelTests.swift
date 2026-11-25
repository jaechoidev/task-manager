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
        // With two projects in the list, a row says which one it is in.
        XCTAssertEqual(stateCaption(issues[0], showRepo: false), "Stopped at verify, pass 2")
        XCTAssertEqual(stateCaption(issues[0], showRepo: true), "r · Stopped at verify, pass 2")
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

    /// What `GET /v1/schedule` answers, in the shape the engine's own test checks.
    let scheduleJSON = """
    {"now":1300,
     "schedulers":[{"pid":4242,"started_at":900,"updated_at":1290,"places":3,"in_order":false,"watch":true,"held_back":null,
       "projects":[
        {"repo":"o/todo","label":"today","max_parallel":null,
         "running":[{"number":45,"key":"stats","title":"Stats","since":1000,"until":null,"waits_on":"","state":"running","stage":"verify","pass":2},
                    {"number":46,"key":"build","title":"Build","since":1240,"until":null,"waits_on":"a free claude call: all 2 are in use","state":"waiting","stage":"review","pass":1}],
         "waiting":[{"number":47,"key":"later","title":"Later","since":null,"until":null,"waits_on":"a free place"}],
         "parked":[{"number":48,"key":"parked","title":"Parked","since":null,"until":4000,"waits_on":"everyone who may develop is out of quota","state":"waiting","stage":"develop","pass":1}],
         "merged":[44],"opened":[],"failed":[]},
        {"repo":"o/counter","label":"today","max_parallel":1,"running":[{"number":3,"key":"counter","title":"Counter","since":1290,"until":null,"waits_on":null}],"waiting":[],"parked":[],"merged":[],"opened":[],"failed":[]}]}],
     "pools":[{"name":"simulator","size":2,"slots":[{"slot":1,"holder":{"repo":"o/todo","number":45,"what":"verify","since":1100}},{"slot":2,"holder":null}]}],
     "out":[{"who":"chatgpt/gpt-x","until":4000,"hits":2,"reason":"You have hit your usage limit"}],
     "quota":[{"provider":"glm","at":1200,"windows":["5-hour window: 1% used"],"out_until":null}]}
    """

    func testTheScheduleDecodesAndSaysWhoHasEachPlace() throws {
        let schedule = try JSONDecoder.api.decode(Schedule.self, from: Data(scheduleJSON.utf8))
        let board = schedule.schedulers[0]
        XCTAssertEqual(board.inUse, 3)
        XCTAssertEqual(placesLine(board), "3 places, 3 in use, projects side by side, watching for new work")
        let todo = board.projects[0]
        // A running issue says its step and how long it has had its place.
        XCTAssertEqual(runningLine(todo.running[0], now: schedule.now), "verify · pass 2 · 5 min 0 s")
        // One that waits inside its place says what for.
        XCTAssertEqual(runningLine(todo.running[1], now: schedule.now), "waiting at review for a free claude call: all 2 are in use · 60 s")
        // One the database has not heard of yet has only just been started.
        XCTAssertEqual(runningLine(board.projects[1].running[0], now: schedule.now), "starting · 10 s")
        XCTAssertEqual(todo.waiting[0].waitsOn, "a free place")
        XCTAssertNil(todo.waiting[0].stage)
        XCTAssertEqual(todo.parked[0].until, 4000)
        XCTAssertEqual(board.projects[1].maxParallel, 1)
        XCTAssertEqual(schedule.pools[0].held, 1)
        XCTAssertEqual(schedule.pools[0].slots[0].holder?.what, "verify")
        XCTAssertNil(schedule.pools[0].slots[1].holder)
        XCTAssertEqual(schedule.out[0].hits, 2)
        XCTAssertNil(schedule.quota[0].outUntil)
    }

    func testSmallPhrasesOfTheSchedule() throws {
        var board = try JSONDecoder.api.decode(Schedule.self, from: Data(scheduleJSON.utf8)).schedulers[0]
        board.inOrder = true
        board.watch = false
        XCTAssertEqual(placesLine(board), "3 places, 3 in use, one project after the other")
        // With one project there is no order to speak of.
        board.projects.removeLast()
        board.places = 1
        XCTAssertEqual(placesLine(board), "1 place, 2 in use")
        XCTAssertEqual(shortRepo("o/todo"), "todo")
        XCTAssertEqual(clockTime(3 * 3600 + 5 * 60, in: TimeZone(identifier: "UTC")!), "03:05")
        let step = { (vendor: String?, model: String?) in StepRow(step: "review", vendor: vendor, model: model, attempt: 1, verdict: "approve", exitCode: 0, seconds: 1, pass: 1) }
        XCTAssertEqual(stepWho(step("glm", "haiku")), "glm · haiku")
        XCTAssertEqual(stepWho(step("codex", nil)), "codex")
        XCTAssertEqual(stepWho(step(nil, nil)), "engine")
    }

    func testTheStripAboveAnIssueSaysHowEachStepWent() {
        let step = { (name: String, verdict: String, pass: Int) in StepRow(step: name, vendor: nil, model: nil, attempt: 1, verdict: verdict, exitCode: 0, seconds: 1, pass: pass) }
        let steps = [step("research", "skipped", 1), step("acceptance", "replied", 1), step("scope", "pass", 1), step("verify", "fail", 1), step("develop", "replied", 2), step("verify", "pass", 2)]
        // With the workflow's own order, a gate that is not a step is left out
        // and a step that has not run yet is still listed.
        let chips = stageChips(steps: steps, order: ["research", "acceptance", "develop", "verify", "review"], current: "review")
        XCTAssertEqual(chips.map(\.name), ["research", "acceptance", "develop", "verify", "review"])
        XCTAssertEqual(chips.map(\.state), ["skip", "ok", "ok", "ok", "current"])
        XCTAssertEqual(chips.map(\.extra), [" skipped", "", "", " ×2", ""])
        XCTAssertEqual(chips.map(\.mark), ["–", "✓", "✓", "✓", "▶"])
        // The last run of a step is what counts, and a step still to come waits.
        let failed = stageChips(steps: Array(steps.prefix(4)), order: ["acceptance", "verify", "review"], current: nil)
        XCTAssertEqual(failed.map(\.state), ["ok", "fail", "pending"])
        // Without a workflow, the steps are listed as they ran.
        XCTAssertEqual(stageChips(steps: steps, order: nil, current: nil).map(\.name), ["research", "acceptance", "scope", "verify", "develop"])
    }

    func testStepsAreGroupedByPassWithWhyEachPassBegan() {
        let step = { (name: String, verdict: String, pass: Int) in StepRow(step: name, vendor: nil, model: nil, attempt: 1, verdict: verdict, exitCode: 0, seconds: 1, pass: pass) }
        let groups = passGroups([step("develop", "replied", 1), step("sync", "merged-base", 1), step("verify", "fail", 2), step("develop", "replied", 3)])
        XCTAssertEqual(groups.map(\.pass), [1, 2, 3])
        XCTAssertEqual(groups.map(\.note), ["2 steps", "after the base branch was merged in", "after a step sent it back"])
        XCTAssertEqual(groups.map { $0.steps.count }, [2, 1, 1])
        XCTAssertEqual(passGroups([step("develop", "replied", 1)]).map(\.note), ["1 step"])
    }

    func testAQuotaLineIsTakenApartForItsBar() {
        XCTAssertEqual(quotaLine("5-hour window: 43% used, resets 2026-11-25 07:00 UTC"), QuotaLine(name: "5-hour window", share: 0.43, used: "43% used", detail: "resets 2026-11-25 07:00 UTC"))
        XCTAssertEqual(quotaLine("weekly window: 100% used, used up, resets soon").share, 1)
        // A line that is not a window has no bar.
        XCTAssertNil(quotaLine("plan: prolite").share)
        XCTAssertNil(quotaLine("5-hour window: close to its limit").share)
        XCTAssertEqual(quotaLine("no colon here").name, "no colon here")
    }

    func testMarksFiltersAndProblemLines() throws {
        let issues = try JSONDecoder.api.decode([Issue].self, from: Data(issuesJSON.utf8))
        XCTAssertEqual(issues.map { mark(of: $0) }, [.needs, .done, .waiting, .pullRequest])
        // An issue the scheduler parked reads differently from one that waits in a step.
        XCTAssertEqual(mark(of: issues[2], parked: true), .parked)
        XCTAssertEqual(issues.filter { matches($0, filter: "", project: nil) }.count, 4)
        XCTAssertEqual(issues.filter { matches($0, filter: "tab ver", project: nil) }.map(\.number), [24])
        XCTAssertEqual(issues.filter { matches($0, filter: "#7", project: nil) }.map(\.number), [7])
        XCTAssertEqual(issues.filter { matches($0, filter: "", project: "o/other") }.count, 0)
        let text = "workflow:\n  - develop: { module: agent/dev }\n  - verify:  { module: check/verifyy }\non_fail:\n  verify: develop\n"
        XCTAssertEqual(problemLine("step 'verify': module 'check/verifyy' does not exist", in: text), 3)
        XCTAssertNil(problemLine("limits: fix_rounds must be at most 5", in: text))
        XCTAssertEqual(refusedSteps(["step 'verify': module 'check/verifyy' does not exist", "step 'review' (agent/review): needs 'verify_log', which no earlier step produces", "limits are wrong"]), ["verify": "module not found", "review": "needs verify_log"])
        // A module is in use only when a step names exactly it.
        XCTAssertTrue(names("agent/dev", in: text))
        XCTAssertFalse(names("check/verify", in: text))
        XCTAssertTrue(names("check/verifyy", in: text))
        XCTAssertEqual(engineLine(address: "http://127.0.0.1:7878", reached: true, updated: Date(timeIntervalSince1970: 100), now: Date(timeIntervalSince1970: 102)), "Engine at 127.0.0.1:7878 · updated 2 s ago")
        XCTAssertEqual(engineLine(address: "http://127.0.0.1:7878", reached: false, updated: nil, now: Date()), "No answer from the engine at 127.0.0.1:7878")
    }

    func testUsageShowsTokensWhenTheEngineCountsThem() throws {
        let json = #"[{"vendor":"claude","calls":48,"seconds":824.0,"rate_limited":0,"tokens_in":2134567,"tokens_cached":1900000,"tokens_out":34012,"token_calls":47},{"vendor":"codex","calls":9,"seconds":301.0,"rate_limited":1}]"#
        let usage = try JSONDecoder.api.decode([UsageRow].self, from: Data(json.utf8))
        XCTAssertEqual(usage[0].tokens, 2_168_579)
        XCTAssertEqual(tokenDetail(usage[0]), "2.1M in (1.9M cached) · 34.0k out")
        // An older engine sends no counts, and a row without them has no total.
        XCTAssertNil(usage[1].tokens)
        XCTAssertEqual(tokenDetail(usage[1]), "")
        XCTAssertEqual([812, 45_321, 123_456, 999_949, 2_134_567, 123_456_789].map(tokenCount), ["812", "45.3k", "123k", "1.0M", "2.1M", "123M"])
    }

    func testAnArrowToAStepThatIsNotThereIsLeftOut() throws {
        var workflow = try workflow()
        workflow.onFail = ["verify": "develop", "review": "nowhere", "acceptance": "develop"]
        let layout = GraphLayout.make(workflow)
        // Only an arrow that goes back to an existing step is drawn.
        XCTAssertEqual(layout.backEdges.map { [$0.from, $0.to] }, [[3, 2]])
    }
}
