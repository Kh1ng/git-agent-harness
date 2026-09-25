import ActivityKit
import Foundation

struct GAHLiveActivityAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        let state: String
        let backend: String
        let model: String
        let startedAt: Int
    }

    let project: String
    let sessionId: String
}
