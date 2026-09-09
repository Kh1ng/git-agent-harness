import XCTest

final class ControllerTests: XCTestCase {
    func testAddressBoundaryAndSecretFreeRestoration() throws {
        for invalid in ["javascript:alert(1)", "file:///etc/passwd", "https://user:secret@example.com", "https://example.com:70000", "gah://open"] {
            XCTAssertThrowsError(try ServerAddress(invalid), invalid)
        }
        let offer = "https://gah.example/#pair=abcdefghijklmnopqrstuvwxyzABCDEF&server=e58dbf8c-9c0d-4bd4-b0f9-be02d42e16a8"
        XCTAssertEqual(try ServerAddress.pairing(offer).origin.absoluteString, "https://gah.example/")
        for invalid in ["https://gah.example/", offer.replacingOccurrences(of: "abcdefghijklmnopqrstuvwxyzABCDEF", with: "short"),
                        offer.replacingOccurrences(of: "e58dbf8c-9c0d-4bd4-b0f9-be02d42e16a8", with: "invalid"),
                        offer + "&pair=abcdefghijklmnopqrstuvwxyzABCDEF", offer + "&server=e58dbf8c-9c0d-4bd4-b0f9-be02d42e16a8"] {
            XCTAssertThrowsError(try ServerAddress.pairing(invalid), invalid)
        }
        let address = try ServerAddress("http://100.118.97.79/#pair=secret&server=identity")
        XCTAssertEqual(ServerAddress.restorationURL(address.url)?.absoluteString, "http://100.118.97.79/")
        XCTAssertTrue(address.contains(URL(string: "http://100.118.97.79:80/?page=nodes")!))
        XCTAssertFalse(address.contains(URL(string: "http://100.118.97.79.example.com/")!))
        XCTAssertFalse(address.contains(URL(string: "https://100.118.97.79/")!))
        let chat = URL(string: "https://gah.example/?page=chat&profile=gah&chat=abc&token=secret#pair=secret")!
        XCTAssertEqual(ServerAddress.restorationURL(chat)?.absoluteString, "https://gah.example/?page=chat&profile=gah&chat=abc")
        let link = URL(string: "gah://open?url=https%3A%2F%2Fgah.example%2F%3Fpage%3Dchat")!
        XCTAssertEqual(try ServerAddress.fromDeepLink(link).url.absoluteString, "https://gah.example/?page=chat")
        XCTAssertThrowsError(try ServerAddress.fromDeepLink(URL(string: "gah://open?url=https://a.example&url=https://b.example")!))
    }

    func testFailedSwitchHidesOldDashboardAndRetryKeepsUnusedPairingCode() throws {
        continueAfterFailure = false
        func control(_ path: String) {
            let completed = expectation(description: path)
            URLSession.shared.dataTask(with: URL(string: "http://127.0.0.1:18773/" + path)!) { _, response, error in
                XCTAssertNil(error)
                XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
                completed.fulfill()
            }.resume()
            wait(for: [completed], timeout: 10)
        }
        control("arm-recovery")
        let app = XCUIApplication()
        app.launchArguments = ["-centralURL", "http://127.0.0.1:18773/"]
        app.launch()
        XCTAssertTrue(app.webViews.staticTexts["GAH controller fixture"].waitForExistence(timeout: 20))
        XCTAssertFalse(app.buttons["connection"].exists)
        app.webViews.links["Open test pairing server"].tap()
        XCTAssertTrue(app.alerts["Open pairing server?"].waitForExistence(timeout: 5))
        app.alerts.buttons["Cancel"].tap()
        XCTAssertTrue(app.webViews.staticTexts["GAH controller fixture"].exists)
        app.webViews.links["Open test pairing server"].tap()
        app.alerts.buttons["Open server"].tap()
        XCTAssertTrue(app.buttons["Retry connection"].waitForExistence(timeout: 20))
        let hidden = XCTAttachment(screenshot: app.screenshot())
        hidden.name = "Failed switch hides previous dashboard"
        hidden.lifetime = .keepAlways
        add(hidden)
        XCTAssertFalse(app.webViews.buttons["Remember test session"].isHittable)
        control("allow-recovery")
        app.buttons["Retry connection"].tap()
        XCTAssertTrue(app.webViews.staticTexts["Pairing fragment retained"].waitForExistence(timeout: 20))
    }

    func testOnlyDashboardSettingsCanOpenScannerWithoutLosingDraft() throws {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments = ["-centralURL", "http://127.0.0.1:18773/"]
        app.launch()
        XCTAssertTrue(app.webViews.staticTexts["GAH controller fixture"].waitForExistence(timeout: 20))
        XCTAssertFalse(app.buttons["connection"].exists)
        let scanner = app.navigationBars["Scan pairing code"]
        app.webViews.buttons["Request scan outside Settings"].tap()
        XCTAssertTrue(app.webViews.staticTexts["Outside request sent"].waitForExistence(timeout: 5))
        XCTAssertFalse(scanner.waitForExistence(timeout: 1))
        app.webViews.buttons["Request scan with duplicate page"].tap()
        XCTAssertTrue(app.webViews.staticTexts["Duplicate page request sent"].waitForExistence(timeout: 5))
        XCTAssertFalse(scanner.waitForExistence(timeout: 1))
        let draft = app.webViews.textViews.firstMatch
        draft.tap()
        draft.typeText("Keep this unsent draft")
        app.webViews.buttons["Settings"].tap()
        for label in ["Request scan from subframe", "Request scan from other origin"] {
            app.webViews.buttons[label].tap()
            XCTAssertTrue(app.webViews.staticTexts[label + " sent"].waitForExistence(timeout: 5))
            XCTAssertFalse(scanner.waitForExistence(timeout: 1))
        }
        app.webViews.buttons["Scan pairing QR code"].tap()
        XCTAssertTrue(scanner.waitForExistence(timeout: 5))
        app.navigationBars.buttons["Cancel"].tap()
        XCTAssertTrue(draft.waitForExistence(timeout: 5))
        XCTAssertEqual(draft.value as? String, "Keep this unsent draft")
        XCTAssertFalse(app.buttons["connection"].exists)
    }

    // Run the local fixture server documented in README before this test. It never contacts a provider.
    func testWebSessionPersistsAndConnectionCanRecover() throws {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments = ["-centralURL", "http://127.0.0.1:18773/"]
        app.launch()
        XCTAssertTrue(app.webViews.staticTexts["GAH controller fixture"].waitForExistence(timeout: 20))
        app.webViews.buttons["Remember test session"].tap()
        XCTAssertTrue(app.webViews.staticTexts["Session retained"].waitForExistence(timeout: 10))
        XCUIDevice.shared.press(.home)
        app.activate()
        XCTAssertTrue(app.webViews.staticTexts["Session retained"].waitForExistence(timeout: 10))
        app.terminate()
        app.launch()
        XCTAssertTrue(app.webViews.staticTexts["Session retained"].waitForExistence(timeout: 20))
        XCTAssertFalse(app.buttons["connection"].exists)
        app.terminate()
        app.launchArguments = ["-centralURL", "http://127.0.0.1:18773/unavailable"]
        app.launch()
        XCTAssertTrue(app.buttons["Retry connection"].waitForExistence(timeout: 20))
        app.buttons["connection"].tap()
        let field = app.descendants(matching: .any).matching(identifier: "serverAddress").firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        field.typeKey("a", modifierFlags: .command)
        field.typeText("javascript:alert(1)")
        app.buttons["connectServer"].tap()
        XCTAssertTrue(app.staticTexts["connectionError"].waitForExistence(timeout: 5))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "Invalid address is rejected"
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }
}
