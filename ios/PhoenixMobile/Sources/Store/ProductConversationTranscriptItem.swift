import Foundation

enum ProductConversationTranscriptItem: Equatable {
    case message(Message)
    case handoff(ProductConversationHandoff)
}
