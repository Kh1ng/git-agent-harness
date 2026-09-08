import XCTest

final class ControllerTests: XCTestCase {
    func testAddressBoundaryAndSecretFreeRestoration() throws {
        for invalid in ["javascript:alert(1)", "file:///etc/passwd", "https://user:secret@example.com", "https://example.com:70000", "gah://open"] {
            XCTAssertThrowsError(try ServerAddress(invalid), invalid)
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
        app.buttons["connection"].tap()
        let field = app.descendants(matching: .any).matching(identifier: "serverAddress").firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.tap()
        field.typeKey("a", modifierFlags: .command)
        field.typeText("http://localhost:18773/recovery#pair=fixture-code")
        XCTAssertEqual(field.value as? String, "http://localhost:18773/recovery#pair=fixture-code")
        app.buttons["connectServer"].tap()
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
