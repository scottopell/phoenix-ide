import Foundation

struct ProductConversationOutboxProjection: Identifiable {
    enum ActionPolicy {
        case readOnly
        case interactive(session: ConversationSession)
    }

    let transcriptRowId: String
    let entry: OutboxEntry
    let actionPolicy: ActionPolicy

    var id: String { "\(transcriptRowId):\(entry.localId)" }
}
