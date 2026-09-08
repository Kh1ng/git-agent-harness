import SwiftUI
import VisionKit

/// Scanning proposes an address; the connection form and server still require confirmation.
struct QRScanner: View {
    @Environment(\.dismiss) private var dismiss
    let scanned: (String) -> Void
    var body: some View {
        NavigationStack {
            Group {
                if DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
                    ScannerCamera(scanned: scanned)
                } else {
                    ContentUnavailableView("Camera unavailable", systemImage: "camera", description:
                        Text("Allow camera access in iPhone Settings, or cancel and paste the pairing link into Connection."))
                }
            }
            .navigationTitle("Scan pairing code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }
    }
}

private struct ScannerCamera: UIViewControllerRepresentable {
    let scanned: (String) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(scanned: scanned) }
    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced, recognizesMultipleItems: false, isGuidanceEnabled: true, isHighlightingEnabled: true)
        scanner.delegate = context.coordinator
        do { try scanner.startScanning() }
        catch { context.coordinator.showFailure(scanner) }
        return scanner
    }
    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {}
    static func dismantleUIViewController(_ scanner: DataScannerViewController, coordinator: Coordinator) { scanner.stopScanning() }
    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let scanned: (String) -> Void
        private var delivered = false
        init(scanned: @escaping (String) -> Void) { self.scanned = scanned }
        func dataScanner(_ scanner: DataScannerViewController, didAdd added: [RecognizedItem], allItems: [RecognizedItem]) {
            guard !delivered else { return }
            for item in added {
                if case .barcode(let barcode) = item, let value = barcode.payloadStringValue {
                    delivered = true
                    scanner.stopScanning()
                    scanned(value)
                    return
                }
            }
        }
        func dataScanner(_ scanner: DataScannerViewController, becameUnavailableWithError error: DataScannerViewController.ScanningUnavailable) {
            showFailure(scanner)
        }
        func showFailure(_ scanner: DataScannerViewController) {
            let label = UILabel()
            label.text = "Camera unavailable. Cancel and paste the pairing link instead."
            label.numberOfLines = 0
            label.textAlignment = .center
            label.font = .preferredFont(forTextStyle: .body)
            label.adjustsFontForContentSizeCategory = true
            label.backgroundColor = .systemBackground
            label.translatesAutoresizingMaskIntoConstraints = false
            scanner.view.addSubview(label)
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: scanner.view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
                label.trailingAnchor.constraint(equalTo: scanner.view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
                label.centerYAnchor.constraint(equalTo: scanner.view.centerYAnchor)
            ])
        }
    }
}
