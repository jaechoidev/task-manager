import Foundation
import UserNotifications

/// macOS notifications. They need an app bundle, so a build run straight from
/// `swift run` stays quiet.
enum Notifier {
    static var available: Bool { Bundle.main.bundleIdentifier != nil }

    static func askOnce() {
        guard available else { return }
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    static func post(title: String, body: String) {
        guard available else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }
}
