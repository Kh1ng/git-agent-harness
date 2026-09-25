import ActivityKit
import SwiftUI
import WidgetKit

@main
struct GAHWidgets: WidgetBundle {
    var body: some Widget { GAHLiveActivityWidget() }
}

struct GAHLiveActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: GAHLiveActivityAttributes.self) { context in
            HStack {
                VStack(alignment: .leading) {
                    Text(context.attributes.project).font(.headline)
                    Text(context.state.state).font(.subheadline)
                    Text([context.state.backend, context.state.model].filter { !$0.isEmpty }.joined(separator: " · ")).font(.caption)
                }
                Spacer()
                Text(timerInterval: Date(timeIntervalSince1970: TimeInterval(context.state.startedAt))...Date.distantFuture, countsDown: false)
                    .font(.caption.monospacedDigit())
            }.padding().widgetURL(chatURL(context.attributes))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) { Text(context.attributes.project).font(.headline) }
                DynamicIslandExpandedRegion(.trailing) { Text(context.state.backend).font(.caption) }
                DynamicIslandExpandedRegion(.bottom) { Text(context.state.state) }
            } compactLeading: {
                Image(systemName: context.state.state.contains("permission") ? "exclamationmark.circle" : "message")
            } compactTrailing: {
                Text(context.state.backend.prefix(3))
            } minimal: {
                Image(systemName: "message")
            }.widgetURL(chatURL(context.attributes))
        }
    }

    private func chatURL(_ attributes: GAHLiveActivityAttributes) -> URL? {
        var components = URLComponents()
        components.scheme = "gah"
        components.host = "chat"
        components.queryItems = [
            URLQueryItem(name: "profile", value: attributes.project),
            URLQueryItem(name: "chat", value: attributes.sessionId)
        ]
        return components.url
    }
}
