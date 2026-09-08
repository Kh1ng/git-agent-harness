import Foundation

/// Validates controller destinations and keeps pairing codes out of saved addresses.
struct ServerAddress: Equatable {
    let url: URL
    let origin: URL

    init(_ value: String) throws {
        guard let parts = URLComponents(string: value.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = parts.scheme?.lowercased(), ["http", "https"].contains(scheme),
              let host = parts.host, !host.isEmpty,
              parts.user == nil, parts.password == nil,
              parts.port.map({ (1...65535).contains($0) }) ?? true,
              let url = parts.url else { throw AddressError.invalid }
        var base = URLComponents()
        base.scheme = scheme
        base.host = host.lowercased()
        base.port = parts.port
        base.path = "/"
        guard let origin = base.url else { throw AddressError.invalid }
        self.url = url
        self.origin = origin
    }

    func contains(_ other: URL) -> Bool {
        guard let candidate = try? ServerAddress(other.absoluteString) else { return false }
        func port(_ url: URL) -> Int { url.port ?? (url.scheme == "https" ? 443 : 80) }
        return origin.scheme == candidate.origin.scheme && origin.host == candidate.origin.host
            && port(origin) == port(candidate.origin)
    }

    /// Persist only dashboard routing fields, never pairing fragments or arbitrary query credentials.
    static func restorationURL(_ url: URL) -> URL? {
        guard let address = try? ServerAddress(url.absoluteString),
              let source = URLComponents(url: url, resolvingAgainstBaseURL: false),
              var result = URLComponents(url: address.origin, resolvingAgainstBaseURL: false) else { return nil }
        let fields = source.queryItems?.filter { ["page", "profile", "chat"].contains($0.name) && ($0.value?.count ?? 0) <= 512 }
        result.queryItems = fields?.isEmpty == false ? fields : nil
        return result.url
    }

    static func fromDeepLink(_ url: URL) throws -> ServerAddress {
        guard url.scheme == "gah", url.host == "open",
              let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let items = parts.queryItems, items.count == 1, items[0].name == "url",
              let value = items[0].value else { throw AddressError.invalid }
        return try ServerAddress(value)
    }

    enum AddressError: LocalizedError {
        case invalid
        var errorDescription: String? { "Enter an HTTP or HTTPS server address without an embedded username or password." }
    }
}
