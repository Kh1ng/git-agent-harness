import SwiftUI
import WebKit

@main
struct GAHApp: App {
    var body: some Scene { WindowGroup { ControllerView() } }
}

/// The web dashboard owns authentication and work. This shell owns only navigation and its persistent web view.
@MainActor
final class Controller: NSObject, ObservableObject, WKNavigationDelegate, WKUIDelegate {
    @Published var address: ServerAddress?
    @Published var error: String?
    @Published var loading = false
    @Published var hasCommittedPage = false
    let webView: WKWebView
    private var locationObservation: NSKeyValueObservation?

    override init() {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        webView = WKWebView(frame: .zero, configuration: configuration)
        super.init()
        webView.navigationDelegate = self
        webView.uiDelegate = self
        webView.allowsBackForwardNavigationGestures = true
        locationObservation = webView.observe(\.url, options: [.new]) { [weak self] _, _ in
            Task { @MainActor [weak self] in self?.rememberLocation() }
        }
        if let saved = UserDefaults.standard.string(forKey: "centralURL"), let restored = try? ServerAddress(saved) {
            connect(restored)
        }
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
            if action.navigationType == .linkActivated && ["https", "http"].contains(url.scheme?.lowercased() ?? "") {
                UIApplication.shared.open(url)
            } else {
                error = "Navigation left your central server. Use Connection to review a different server address."
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

private struct Dashboard: UIViewRepresentable {
    let controller: Controller
    func makeUIView(context: Context) -> WKWebView { controller.webView }
    func updateUIView(_ view: WKWebView, context: Context) {}
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
                    }.padding().frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color(uiColor: .secondarySystemBackground))
                }
                if controller.address != nil {
                    Dashboard(controller: controller)
                        .opacity(controller.hasCommittedPage ? 1 : 0)
                        .allowsHitTesting(controller.hasCommittedPage)
                        .accessibilityHidden(!controller.hasCommittedPage)
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
            .navigationTitle("GAH")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Connection", systemImage: "network") {
                        proposedAddress = nil
                        showingConnection = true
                    }.accessibilityIdentifier("connection")
                }
            }
            .sheet(isPresented: $showingConnection) {
                ConnectionView(initial: proposedAddress ?? controller.address?.origin.absoluteString ?? "http://100.118.97.79") { target in
                    controller.connect(target)
                    proposedAddress = nil
                    showingConnection = false
                }.id(proposedAddress)
            }
            .onOpenURL { url in
                do {
                    proposedAddress = try ServerAddress.fromDeepLink(url).url.absoluteString
                    showingConnection = true
                } catch { controller.error = "This GAH link is invalid. Open Connection and paste a central server or pairing link." }
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
                } header: { Text("Central server") } footer: {
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
            .navigationTitle("Connection")
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
