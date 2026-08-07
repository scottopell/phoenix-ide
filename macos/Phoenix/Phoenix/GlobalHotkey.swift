import Carbon
import Cocoa

final class GlobalHotkeyManager {
    private var hotKeyRef: EventHotKeyRef?
    private var eventHandlerRef: EventHandlerRef?
    private var actionContext: UnsafeMutablePointer<() -> Void>?

    deinit {
        if let hotKeyRef { UnregisterEventHotKey(hotKeyRef) }
        if let eventHandlerRef { RemoveEventHandler(eventHandlerRef) }
        actionContext?.deinitialize(count: 1)
        actionContext?.deallocate()
    }

    func register(action: @escaping () -> Void) -> Result<Void, HotkeyError> {
        let hotKeyID = EventHotKeyID(signature: OSType(0x5048_4F58), id: 1)
        var eventType = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: UInt32(kEventHotKeyPressed)
        )
        let context = UnsafeMutablePointer<() -> Void>.allocate(capacity: 1)
        context.initialize(to: action)
        actionContext = context

        let handlerStatus = InstallEventHandler(
            GetApplicationEventTarget(),
            { _, _, userData -> OSStatus in
                guard let userData else { return OSStatus(eventNotHandledErr) }
                userData.assumingMemoryBound(to: (() -> Void).self).pointee()
                return noErr
            },
            1,
            &eventType,
            context,
            &eventHandlerRef
        )
        guard handlerStatus == noErr else { return .failure(.registrationFailed(handlerStatus)) }

        let modifiers = UInt32(cmdKey | controlKey)
        let status = RegisterEventHotKey(0x11, modifiers, hotKeyID, GetApplicationEventTarget(), 0, &hotKeyRef)
        guard status == noErr else { return .failure(.registrationFailed(status)) }
        return .success(())
    }
}

enum HotkeyError: LocalizedError {
    case registrationFailed(OSStatus)

    var errorDescription: String? {
        switch self {
        case .registrationFailed(let status): "The global shortcut Control-Command-T could not be registered (OSStatus \(status))."
        }
    }
}
