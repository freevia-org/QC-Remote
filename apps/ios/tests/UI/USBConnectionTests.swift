import XCTest

final class USBConnectionTests: XCTestCase {
    override func setUp() { continueAfterFailure = false }

    private func refreshPairedHost(_ app: XCUIApplication) {
        let refresh = app.buttons["Refresh host state"]
        XCTAssertTrue(refresh.waitForExistence(timeout: 45), app.debugDescription)
        refresh.tap()
    }

    func testPairedHostModelCatalog() throws {
        guard let preset = ProcessInfo.processInfo.environment["QC_EXPECTED_PRESET"] else {
            throw XCTSkip("Requires a paired live host")
        }
        let app = XCUIApplication(bundleIdentifier: "org.freevia.qcremote")
        app.launch()
        refreshPairedHost(app)
        XCTAssertTrue(app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", preset)).firstMatch.waitForExistence(timeout: 90))
        let empty = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@", "Add a block at row 2, column 1")).firstMatch
        XCTAssertTrue(empty.waitForExistence(timeout: 15))
        // WebKit includes the preceding SVG lane in this accessibility frame.
        // Its lower portion is the empty cell’s actual touch target.
        empty.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.9)).tap()
        // Read only: never select a model or pin anything on the hardware.
        XCTAssertTrue(app.buttons["Brit 2203"].waitForExistence(timeout: 90), app.debugDescription)
        let close = app.buttons.matching(NSPredicate(format: "label == %@", "Close device browser"))
        for index in 0..<close.count where close.element(boundBy: index).isHittable {
            close.element(boundBy: index).tap()
            break
        }
        XCTAssertFalse(app.otherElements["TopBrowserBar"].exists)
    }

    // Opt-in live control check: two recalls only; no saves, rename or settings writes.
    func testPairedHostPresetControls() throws {
        guard let preset = ProcessInfo.processInfo.environment["QC_EXPECTED_PRESET"],
              let previous = ProcessInfo.processInfo.environment["QC_EXPECTED_PREVIOUS_PRESET"] else {
            throw XCTSkip("Requires explicit live preset-navigation test configuration")
        }
        let app = XCUIApplication(bundleIdentifier: "org.freevia.qcremote")
        app.launch()
        refreshPairedHost(app)
        let up = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@", "Previous preset encoder footswitch;")).firstMatch
        let down = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@", "Next preset encoder footswitch;")).firstMatch
        let original = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@ AND label ENDSWITH %@", "Open preset Directory; current preset ", preset)).firstMatch
        XCTAssertTrue(original.waitForExistence(timeout: 90), app.debugDescription)
        XCTAssertTrue(up.waitForExistence(timeout: 15))
        up.tap()
        let recalled = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@ AND label ENDSWITH %@", "Open preset Directory; current preset ", previous)).firstMatch
        XCTAssertTrue(recalled.waitForExistence(timeout: 90), app.debugDescription)
        down.tap()
        XCTAssertTrue(original.waitForExistence(timeout: 90), app.debugDescription)
        XCTAssertFalse(app.otherElements["TopBrowserBar"].exists)
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "Paired-host UP and DOWN restored original preset"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    func testPairedHostRestoresSavedSignIn() throws {
        guard let preset = ProcessInfo.processInfo.environment["QC_EXPECTED_PRESET"] else {
            throw XCTSkip("Requires a paired live host and TEST_RUNNER_QC_EXPECTED_PRESET")
        }
        let app = XCUIApplication(bundleIdentifier: "org.freevia.qcremote")
        for launch in 1...2 {
            app.launch()
            let paired = app.descendants(matching: .any).matching(NSPredicate(format: "label == %@", "Paired host")).firstMatch
            XCTAssertTrue(paired.waitForExistence(timeout: 45))
            paired.tap()
            refreshPairedHost(app)
            let livePreset = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", preset)).firstMatch
            XCTAssertTrue(livePreset.waitForExistence(timeout: 60), "Live preset missing after launch \(launch): \(app.debugDescription)")
            XCTAssertFalse(app.otherElements["TopBrowserBar"].exists, "Saved sign-in unexpectedly reopened authorization")
            XCTAssertFalse(app.staticTexts["The preset or device session changed before this operation completed."].exists, "Explicit refresh must adopt the target without a competing connection attempt")
            let attachment = XCTAttachment(screenshot: app.screenshot())
            attachment.name = "Live QC after launch \(launch)"
            attachment.lifetime = .keepAlways
            add(attachment)
            app.terminate()
        }
    }

    // iPad Simulator has no usable direct USB provider. This checks the local
    // UI demo assets only; iOS does not expose that local demo through MCP.
    func testIPadLocalDemoAssetsAndReturnToPairedHost() throws {
        guard ProcessInfo.processInfo.environment["QC_IPAD_LOCAL_DEMO_TEST"] == "1",
              let preset = ProcessInfo.processInfo.environment["QC_EXPECTED_PRESET"] else {
            throw XCTSkip("Requires explicit iPad local-demo validation")
        }
        let app = XCUIApplication(bundleIdentifier: "org.freevia.qcremote")
        app.launch()
        let direct = app.descendants(matching: .any).matching(NSPredicate(format: "label == %@", "Direct USB when available")).firstMatch
        XCTAssertTrue(direct.waitForExistence(timeout: 45))
        direct.tap()
        let local = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@", "LOCAL DEMO;")).firstMatch
        let localReady = local.waitForExistence(timeout: 60)
        let demo = XCTAttachment(screenshot: app.screenshot())
        demo.name = "Local iPad demo uses bundled assets; not an MCP server"
        demo.lifetime = .keepAlways
        add(demo)
        let browserOpened = app.otherElements["TopBrowserBar"].exists
        // Restore the original route before checking the local-demo assertion.
        let paired = app.descendants(matching: .any).matching(NSPredicate(format: "label == %@", "Paired host")).firstMatch
        XCTAssertTrue(paired.waitForExistence(timeout: 15))
        paired.tap()
        refreshPairedHost(app)
        let restored = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH %@ AND label ENDSWITH %@", "Open preset Directory; current preset ", preset)).firstMatch.waitForExistence(timeout: 90)
        XCTAssertTrue(localReady, "The bundled local demo did not become ready")
        XCTAssertFalse(browserOpened)
        XCTAssertTrue(restored, "Paired-host sign-in did not restore after the local demo")
        XCTAssertFalse(app.otherElements["TopBrowserBar"].exists)
    }

}
