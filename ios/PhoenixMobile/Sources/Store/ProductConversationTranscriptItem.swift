import Foundation

enum ProductConversationTranscriptItem: Equatable, Identifiable {
    var id: String {
        switch self {
        case .message(let message): "message:\(message.conversation_id ?? "unknown"):\(message.message_id)"
        case .handoff(.completed(let predecessor, let successor, let continuation, _, _)):
            "handoff:\(predecessor):\(successor):\(continuation)"
        case .handoff(.historical(let predecessor, let successor, let continuation, _)):
            "handoff:\(predecessor):\(successor):\(continuation)"
        }
    }

    case message(Message)
    case handoff(ProductConversationHandoff)
}
