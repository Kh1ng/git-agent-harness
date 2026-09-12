import Foundation

struct ActivityNotificationRequest: Equatable {
    let id: String
    let title: String
    let body: String
}

func activityNotificationRequest(from value: Any) -> ActivityNotificationRequest? {
    guard let value = value as? [String: Any], value["type"] as? String == "activity",
          let id = value["id"] as? String, let title = value["title"] as? String,
          let body = value["body"] as? String, !id.isEmpty, id.count <= 128,
          !title.isEmpty, title.count <= 120, !body.isEmpty, body.count <= 500,
          !id.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
          !title.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains),
          !body.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains) else { return nil }
    return ActivityNotificationRequest(id: id, title: title, body: body)
}
