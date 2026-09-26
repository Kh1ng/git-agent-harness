import SwiftUI
import ActivityKit
import UserNotifications
import WebKit

@main
struct GAHApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    var body: some Scene { WindowGroup { ControllerView() } }
}

final class AppDelegate: NSObject, UIApplicationDelegate {
    weak static var controller: Controller?

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Task { @MainActor in AppDelegate.controller?.setDeviceToken(deviceToken.hex) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        print("APNs registration failed: \(error.localizedDescription)")
    }
}

private extension Data {
    var hex: String { map { String(format: "%02x", $0) }.joined() }
}

/// The web dashboard owns authentication and work. This shell owns only navigation and its persistent web view.
@MainActor
final class Controller: NSObject, ObservableObject, WKNavigationDelegate, WKUIDelegate, UNUserNotificationCenterDelegate {
    private static let notificationsEnabledKey = "backgroundNotificationsEnabled"
    @Published var address: ServerAddress?
    @Published var error: String?
    @Published var loading = false
    @Published var scanning = false
    @Published var pairingDestination: ServerAddress?
    private var hasCommittedPage = false {
        didSet {
            webView.isHidden = !hasCommittedPage
            webView.isUserInteractionEnabled = hasCommittedPage
            webView.accessibilityElementsHidden = !hasCommittedPage
        }
    }
    let webView: WKWebView
    private var locationObservation: NSKeyValueObservation?
    private var deviceToken: String?
    private var pushToStartToken: String?

    override init() {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        webView = WKWebView(frame: .zero, configuration: configuration)
        super.init()
        configuration.userContentController.add(DashboardRequestHandler(controller: self), name: "gahController")
        UNUserNotificationCenter.current().delegate = self
        AppDelegate.controller = self
        if notificationsEnabled { UIApplication.shared.registerForRemoteNotifications() }
        webView.isHidden = true
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.allowsBackForwardNavigationGestures = true
        locationObservation = webView.observe(\.url, options: [.new]) { [weak self] _, _ in
            Task { @MainActor [weak self] in self?.rememberLocation() }
        }
        if let saved = UserDefaults.standard.string(forKey: "centralURL"), let restored = try? ServerAddress(saved) {
            connect(restored)
        }
        if !notificationsEnabled { removePushRegistration() }
        observeLiveActivityTokens()
    }

    private var notificationsEnabled: Bool {
        UserDefaults.standard.object(forKey: Self.notificationsEnabledKey) as? Bool ?? true
    }

    /// Only the configured dashboard's main Settings page can request the camera.
    func requestPairingScan(_ message: WKScriptMessage) {
        guard message.name == "gahController", message.body as? String == "scanPairingCode",
              message.webView === webView, message.frameInfo.isMainFrame,
              let address, let sender = message.frameInfo.request.url, address.contains(sender),
              let current = webView.url, address.contains(current),
              URLComponents(url: current, resolvingAgainstBaseURL: false)?.queryItems?.first(where: {
                  $0.name == "page"
              })?.value == "settings" else { return }
        let security = message.frameInfo.securityOrigin
        var origin = URLComponents()
        origin.scheme = security.protocol
        origin.host = security.host
        if security.port != 0 { origin.port = security.port }
        guard let source = origin.url, address.contains(source) else { return }
        scanning = true
    }

    func requestNotificationAccess(_ message: WKScriptMessage) {
        guard validDashboardMessage(message),
              let body = message.body as? [String: Any], body["type"] as? String == "requestNotifications" else { return }
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { [weak self] granted, _ in
            Task { @MainActor [weak self] in
                UserDefaults.standard.set(granted, forKey: Self.notificationsEnabledKey)
                if granted {
                    UIApplication.shared.registerForRemoteNotifications()
                } else {
                    UIApplication.shared.unregisterForRemoteNotifications()
                    self?.removePushRegistration()
                }
                self?.webView.evaluateJavaScript(
                    "window.dispatchEvent(new CustomEvent('gah:notification-permission',{detail:{granted:\(granted)}}))"
                )
            }
        }
    }

    func postActivityNotification(_ message: WKScriptMessage) {
        guard validDashboardMessage(message), let request = activityNotificationRequest(from: message.body) else { return }
        let content = UNMutableNotificationContent()
        content.title = request.title
        content.body = request.body
        content.sound = .default
        if let url = request.url { content.userInfo["url"] = url }
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: request.id, content: content, trigger: nil))
    }

    func setDeviceToken(_ token: String) {
        deviceToken = token
        if notificationsEnabled { registerPushTokens() }
    }

    func requestSignOut(_ message: WKScriptMessage) {
        guard validDashboardMessage(message) else { return }
        removePushRegistration(notifyWebView: true)
    }

    func disableNotifications(_ message: WKScriptMessage) {
        guard validDashboardMessage(message) else { return }
        UserDefaults.standard.set(false, forKey: Self.notificationsEnabledKey)
        UIApplication.shared.unregisterForRemoteNotifications()
        removePushRegistration()
    }

    private func removePushRegistration(notifyWebView: Bool = false) {
        guard let id = UserDefaults.standard.string(forKey: "apnsDeviceId") else {
            if notifyWebView { finishPushSignOut() }
            return
        }
        authenticatedRequest(path: "/api/push/apns-devices/\(id)", method: "DELETE", body: nil) { [weak self] _, succeeded in
            if succeeded { UserDefaults.standard.removeObject(forKey: "apnsDeviceId") }
            if notifyWebView { self?.finishPushSignOut() }
        }
    }

    private func finishPushSignOut() {
        webView.evaluateJavaScript("window.dispatchEvent(new Event('gah:push-signout-complete'))")
    }

    private func observeLiveActivityTokens() {
        guard #available(iOS 17.2, *) else { return }
        Task { [weak self] in
            for await token in Activity<GAHLiveActivityAttributes>.pushToStartTokenUpdates {
                await MainActor.run { self?.pushToStartToken = token.hex; self?.registerPushTokens() }
            }
        }
        Task { [weak self] in
            for await activity in Activity<GAHLiveActivityAttributes>.activityUpdates {
                Task {
                    for await token in activity.pushTokenUpdates {
                        await MainActor.run {
                            self?.registerPushTokens(liveActivity: [
                                "profile": activity.attributes.project,
                                "sessionId": activity.attributes.sessionId,
                                "token": token.hex
                            ])
                        }
                    }
                }
            }
        }
    }

    private func registerPushTokens(liveActivity: [String: String]? = nil) {
        guard notificationsEnabled, let deviceToken else { return }
        var body: [String: Any] = ["token": deviceToken, "label": UIDevice.current.name]
        if let pushToStartToken { body["pushToStartToken"] = pushToStartToken }
        if let liveActivity { body["liveActivity"] = liveActivity }
        authenticatedRequest(path: "/api/push/apns-devices", method: "POST", body: body) { [weak self] response, succeeded in
            guard succeeded, let response, let id = (try? JSONSerialization.jsonObject(with: response)) as? [String: Any] else { return }
            if let id = id["id"] as? String {
                UserDefaults.standard.set(id, forKey: "apnsDeviceId")
                if self?.notificationsEnabled == false { self?.removePushRegistration() }
            }
        }
    }

    private func authenticatedRequest(path: String, method: String, body: [String: Any]?, completion: @escaping (Data?, Bool) -> Void) {
        guard let address, let url = URL(string: path, relativeTo: address.origin)?.absoluteURL else {
            completion(nil, false)
            return
        }
        webView.configuration.websiteDataStore.httpCookieStore.getAllCookies { cookies in
            let scopedCookies = cookies.filter { cookie in
                let domain = cookie.domain.trimmingCharacters(in: CharacterSet(charactersIn: "."))
                return (url.host == domain || url.host?.hasSuffix("." + domain) == true)
                    && url.path.hasPrefix(cookie.path) && (!cookie.isSecure || url.scheme == "https")
            }
            var request = URLRequest(url: url)
            request.timeoutInterval = 8
            request.httpMethod = method
            request.setValue(UUID().uuidString, forHTTPHeaderField: "Idempotency-Key")
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.setValue(HTTPCookie.requestHeaderFields(with: scopedCookies)["Cookie"], forHTTPHeaderField: "Cookie")
            request.setValue(address.origin.absoluteString, forHTTPHeaderField: "Origin")
            if let body { request.httpBody = try? JSONSerialization.data(withJSONObject: body) }
            URLSession.shared.dataTask(with: request) { data, response, _ in
                let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                DispatchQueue.main.async { completion(data, (200..<300).contains(status)) }
            }.resume()
        }
    }

    private func validDashboardMessage(_ message: WKScriptMessage) -> Bool {
        guard message.name == "gahController", message.webView === webView, message.frameInfo.isMainFrame,
              let address, let sender = message.frameInfo.request.url, address.contains(sender),
              let current = webView.url, address.contains(current) else { return false }
        let security = message.frameInfo.securityOrigin
        var origin = URLComponents()
        origin.scheme = security.protocol
        origin.host = security.host
        if security.port != 0 { origin.port = security.port }
        return origin.url.map(address.contains) == true
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                             withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                             withCompletionHandler completionHandler: @escaping () -> Void) {
        let path = response.notification.request.content.userInfo["url"] as? String
        Task { @MainActor [weak self] in
            if let self, let path, path.hasPrefix("/?"), let address = self.address,
               let target = URL(string: path, relativeTo: address.origin)?.absoluteURL, address.contains(target) {
                self.webView.load(URLRequest(url: target))
            }
            completionHandler()
        }
    }

    func scannedPairing(_ value: String) {
        scanning = false
        do {
            let target = try ServerAddress.pairing(value)
            if address?.contains(target.url) == true { connect(target) }
            else { pairingDestination = target }
        } catch { self.error = error.localizedDescription }
    }

    func connect(_ target: ServerAddress) {
        webView.stopLoading()
        address = target
        hasCommittedPage = false
        error = nil
        UserDefaults.standard.set(ServerAddress.restorationURL(target.url)?.absoluteString, forKey: "centralURL")
        webView.load(URLRequest(url: target.url))
    }

    func retry() {
        error = nil
        guard let target = address else { return }
        // Keep an unused pairing URL in memory after a failed first load. Once loaded,
        // the dashboard removes its fragment before redemption; retry uses that current URL.
        let current = hasCommittedPage ? webView.url.flatMap { target.contains($0) ? $0 : nil } : nil
        webView.load(URLRequest(url: current ?? target.url))
    }

    func openChatLink(_ link: URL) -> Bool {
        guard let url = address?.chatURL(from: link) else { return false }
        webView.load(URLRequest(url: url))
        return true
    }

    func rememberLocation() {
        guard let current = webView.url, address?.contains(current) == true else { return }
        UserDefaults.standard.set(ServerAddress.restorationURL(current)?.absoluteString, forKey: "centralURL")
    }

    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = action.request.url else { decisionHandler(.cancel); return }
        // Embedded previews remain web content. Only the configured origin may become the controller's main document.
        if action.targetFrame?.isMainFrame == false { decisionHandler(.allow); return }
        if address?.contains(url) == true {
            if action.targetFrame == nil { webView.load(action.request); decisionHandler(.cancel) }
            else { decisionHandler(.allow) }
        } else {
            decisionHandler(.cancel)
            if action.sourceFrame.isMainFrame,
               let source = action.sourceFrame.request.url, address?.contains(source) == true,
               let pairing = try? ServerAddress.pairing(url.absoluteString) {
                pairingDestination = pairing
            } else if action.navigationType == .linkActivated && ["https", "http"].contains(url.scheme?.lowercased() ?? "") {
                UIApplication.shared.open(url)
            } else {
                error = "Navigation left your central server. Open connection settings to review a different server address."
            }
        }
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) { loading = true; error = nil }
    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        hasCommittedPage = webView.url.map { address?.contains($0) == true } ?? false
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) { loading = false; rememberLocation() }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError failure: Error) { failed(failure) }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError failure: Error) { failed(failure) }
    private func failed(_ failure: Error) {
        guard (failure as NSError).code != NSURLErrorCancelled else { return }
        loading = false
        error = "Cannot load your central server. Check Tailscale and the server address, then retry."
    }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        loading = false
        error = "The dashboard was closed by iOS. Retry to restore saved chat history. Unsent text may need to be entered again."
    }
}

/// WebKit retains handlers, so the handler must not retain the controller and its web view.
@MainActor
private final class DashboardRequestHandler: NSObject, WKScriptMessageHandler {
    weak var controller: Controller?
    init(controller: Controller) { self.controller = controller }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        if message.body as? String == "scanPairingCode" { controller?.requestPairingScan(message); return }
        guard let body = message.body as? [String: Any], let type = body["type"] as? String else { return }
        if type == "requestNotifications" { controller?.requestNotificationAccess(message) }
        else if type == "activity" { controller?.postActivityNotification(message) }
        else if type == "disableNotifications" { controller?.disableNotifications(message) }
        else if type == "signOut" { controller?.requestSignOut(message) }
    }
}

private struct Dashboard: UIViewRepresentable {
    let controller: Controller
    func makeUIView(context: Context) -> UIView {
        // SwiftUI manages the host's visibility. The controller hides its child web view
        // independently while a server switch has not committed a new document.
        let host = UIView()
        let webView = controller.webView
        webView.translatesAutoresizingMaskIntoConstraints = false
        host.addSubview(webView)
        NSLayoutConstraint.activate([
            webView.leadingAnchor.constraint(equalTo: host.leadingAnchor),
            webView.trailingAnchor.constraint(equalTo: host.trailingAnchor),
            webView.topAnchor.constraint(equalTo: host.topAnchor),
            webView.bottomAnchor.constraint(equalTo: host.bottomAnchor)
        ])
        return host
    }
    func updateUIView(_ view: UIView, context: Context) {}
}

private struct ControllerView: View {
    @StateObject private var controller = Controller()
    @Environment(\.scenePhase) private var scenePhase
    @State private var showingConnection = false
    @State private var proposedAddress: String?

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if controller.loading { ProgressView("Loading dashboard").padding(.vertical, 8) }
                if let error = controller.error {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(error).foregroundStyle(.primary)
                        Button("Retry connection") { controller.retry() }.frame(minHeight: 44)
                        Button("Connection settings") { proposedAddress = nil; showingConnection = true }
                            .frame(minHeight: 44).accessibilityIdentifier("connection")
                    }.padding().frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color(uiColor: .secondarySystemBackground))
                }
                if controller.address != nil {
                    Dashboard(controller: controller)
                } else {
                    ContentUnavailableView {
                        Label("Connect to GAH", systemImage: "network")
                    } description: {
                        Text("Manage projects, chats, and worker nodes through your central server. Turn on Tailscale to reach your private network.")
                    } actions: {
                        Button("Set up connection") { showingConnection = true }
                            .buttonStyle(.borderedProminent).frame(minHeight: 44)
                    }
                }
            }
            .toolbar(.hidden, for: .navigationBar)
            .sheet(isPresented: $controller.scanning) {
                QRScanner { controller.scannedPairing($0) }
            }
            .alert("Open pairing server?", isPresented: Binding(
                get: { controller.pairingDestination != nil },
                set: { if !$0 { controller.pairingDestination = nil } }
            ), presenting: controller.pairingDestination) { target in
                Button("Open server") { controller.connect(target) }
                Button("Cancel", role: .cancel) {}
            } message: { target in
                Text("Open \(target.origin.absoluteString)? You will confirm this server before pairing.")
            }
            .sheet(isPresented: $showingConnection) {
                ConnectionView(initial: proposedAddress ?? controller.address?.origin.absoluteString ?? "http://100.118.97.79") { target in
                    controller.connect(target)
                    proposedAddress = nil
                    showingConnection = false
                }.id(proposedAddress)
            }
            .onOpenURL { url in
                if controller.openChatLink(url) { return }
                do {
                    proposedAddress = try ServerAddress.fromDeepLink(url).url.absoluteString
                    showingConnection = true
                } catch { controller.error = "This GAH link is invalid. Open connection settings and paste a central server or pairing link." }
            }
            .onChange(of: scenePhase) { _, phase in
                if phase != .active { controller.rememberLocation() }
                // WebSocket reconnect belongs to the dashboard. Do not reload it and destroy an unsent draft on resume.
            }
        }
    }
}

private struct ConnectionView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var text: String
    @State private var error: String?
    @State private var scanning = false
    let connect: (ServerAddress) -> Void

    init(initial: String, connect: @escaping (ServerAddress) -> Void) {
        _text = State(initialValue: initial)
        self.connect = connect
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Central address or pairing link", text: $text, axis: .vertical)
                        .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                        .accessibilityIdentifier("serverAddress")
                    Button("Scan pairing QR code", systemImage: "qrcode.viewfinder") { scanning = true }
                } header: { Text("Connection & pairing") } footer: {
                    Text("Paste a server address or a pairing link from GAH. Review the address before connecting. Your phone controls work on the server and its nodes.")
                }
                if let target = try? ServerAddress(text) {
                    Section("Connect to") {
                        Text(target.origin.absoluteString).textSelection(.enabled)
                        if target.origin.scheme == "http" {
                            Text("Use HTTP only over Tailscale or a trusted network.").foregroundStyle(.secondary)
                        }
                    }
                }
                if let error { Section { Text(error).foregroundStyle(.red).accessibilityIdentifier("connectionError") } }
                Section {
                    Button("Connect") {
                        do { connect(try ServerAddress(text)) }
                        catch { self.error = error.localizedDescription }
                    }.accessibilityIdentifier("connectServer")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
            .sheet(isPresented: $scanning) {
                QRScanner { value in
                    text = value
                    error = nil
                    scanning = false
                }
            }
        }
    }
}
