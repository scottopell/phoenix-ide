import Foundation

enum ProductConversationSessionEvent: Equatable {
    case aggregateTopologyInvalidated(ProductConversationTopologyInvalidation)
    case connectionChanged(ConversationSession.ConnectionState)
    case messagesChanged
    case outboxChanged
    case errorToastChanged(String?)
}

struct ProductConversationTopologyInvalidation: Equatable {
    let transcriptRowId: String
    let aggregateIdentity: String
    let reason: Reason

    enum Reason: Equatable {
        case contextExhausted
        case awaitingContinuation
        case handedOff(successorConversationId: String?)
        case terminal
        case aggregateIdentityChanged(previous: String, current: String)
    }
}

extension ConversationState {
    var productConversationTopologyInvalidationReason: ProductConversationTopologyInvalidation.Reason? {
        switch self {
        case .contextExhausted:
            .contextExhausted
        case .awaitingContinuation:
            .awaitingContinuation
        case .handedOff(let successorConversationId):
            .handedOff(successorConversationId: successorConversationId)
        case .terminal:
            .terminal
        default:
            nil
        }
    }
}
