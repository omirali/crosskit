import AppKit
import Carbon.HIToolbox

/// Глобальная клавиша через Carbon `RegisterEventHotKey`.
///
/// Выбран именно этот путь, а не `NSEvent.addGlobalMonitorForEvents` и не `CGEventTap`:
/// оба последних читают чужой ввод и потому требуют разрешения Accessibility.
/// `RegisterEventHotKey` регистрирует одно сочетание у оконного сервера и не видит
/// ничего, кроме него, — разрешения система за это не спрашивает.
final class GlobalHotkey {

    enum Failure: Error, CustomStringConvertible {
        case handlerNotInstalled(OSStatus)
        case notRegistered(OSStatus)

        var description: String {
            switch self {
            case .handlerNotInstalled(let status): return "InstallEventHandler отказал: OSStatus \(status)"
            case .notRegistered(let status): return "RegisterEventHotKey отказал: OSStatus \(status)"
            }
        }
    }

    /// Момент нажатия по часам события, а не по часам обработчика: `GetEventTime`
    /// возвращает штамп, поставленный системой при появлении события.
    private let onPress: (EventTime) -> Void
    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?

    init(keyCode: UInt32, modifiers: UInt32, signature: OSType, onPress: @escaping (EventTime) -> Void) throws {
        self.onPress = onPress

        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let context = Unmanaged.passUnretained(self).toOpaque()
        let installed = InstallEventHandler(GetEventDispatcherTarget(), { _, event, context in
            guard let event, let context else { return OSStatus(eventNotHandledErr) }
            let owner = Unmanaged<GlobalHotkey>.fromOpaque(context).takeUnretainedValue()
            owner.onPress(GetEventTime(event))
            return noErr
        }, 1, &spec, context, &handler)
        guard installed == noErr else { throw Failure.handlerNotInstalled(installed) }

        let id = EventHotKeyID(signature: signature, id: 1)
        let registered = RegisterEventHotKey(keyCode, modifiers, id, GetEventDispatcherTarget(), 0, &hotKey)
        guard registered == noErr else { throw Failure.notRegistered(registered) }
    }

    deinit {
        if let hotKey { UnregisterEventHotKey(hotKey) }
        if let handler { RemoveEventHandler(handler) }
    }
}
