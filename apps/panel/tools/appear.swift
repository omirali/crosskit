// Независимая проверка замера — снаружи процесса панели.
//
// Отправляет ⌥Space и опрашивает оконный сервер, пока окно панели не станет экранным.
// Приложение о существовании этой программы не знает, и она не знает о его часах: обе
// стороны меряют одно и то же разными способами, и расхождение между ними видно.
//
// Второй режим, `type`, печатает строку в то окно, которое сейчас ключевое. Он нужен для
// скриншота: обычный `keystroke` из AppleScript проходит через текущую раскладку и
// кириллицу превращает в латиницу, а Unicode-событие раскладки не спрашивает.
//
// Сборка и запуск (нужны разрешения «Универсальный доступ» и «Запись экрана» тому,
// кто запускает, — подделка ввода и чтение списка окон):
//     swiftc -O apps/panel/tools/appear.swift -o /tmp/appear
//     /tmp/appear 8            # восемь кругов замера
//     /tmp/appear type «текст» # напечатать в ключевое окно

import ApplicationServices
import CoreGraphics
import Foundation

let panelOwner = "CrosskitPanel"

func panelIsOnScreen() -> Bool {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
    return windows.contains { ($0[kCGWindowOwnerName as String] as? String) == panelOwner }
}

/// Отправляет только нажатие: горячая клавиша срабатывает на нём, и всё, что делается
/// после, в замер попасть не должно. Отпускание отправляется отдельно.
func postOptionSpace(down: Bool) {
    let source = CGEventSource(stateID: .hidSystemState)
    guard let event = CGEvent(keyboardEventSource: source, virtualKey: 49, keyDown: down) else { return }
    event.flags = .maskAlternate
    event.post(tap: .cghidEventTap)
}

func pressOptionSpace() {
    postOptionSpace(down: true)
    usleep(20_000)
    postOptionSpace(down: false)
}

guard AXIsProcessTrusted() else {
    print("нет разрешения «Универсальный доступ»: нажатие подделать нельзя")
    exit(1)
}
guard CGPreflightScreenCaptureAccess() else {
    print("нет разрешения «Запись экрана»: список окон читается без имён")
    exit(1)
}

/// Печатает строку целиком одним Unicode-событием, минуя раскладку клавиатуры.
func type(_ text: String) {
    let source = CGEventSource(stateID: .hidSystemState)
    var utf16 = Array(text.utf16)
    for down in [true, false] {
        guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else { continue }
        event.keyboardSetUnicodeString(stringLength: utf16.count, unicodeString: &utf16)
        event.post(tap: .cghidEventTap)
        usleep(20_000)
    }
}

let arguments = Array(CommandLine.arguments.dropFirst())
if arguments.first == "type" {
    type(arguments.dropFirst().joined(separator: " "))
    exit(0)
}

let rounds = Int(arguments.first ?? "") ?? 8
for round in 1...rounds {
    while panelIsOnScreen() {
        pressOptionSpace()
        usleep(400_000)
    }
    var appeared: TimeInterval?
    let start = Date()
    postOptionSpace(down: true)
    while Date().timeIntervalSince(start) < 2 {
        if panelIsOnScreen() {
            appeared = Date().timeIntervalSince(start)
            break
        }
    }
    postOptionSpace(down: false)
    if let appeared {
        print(String(format: "круг %d: окно стало экранным через %.1f мс после отправки нажатия",
                     round, appeared * 1000))
    } else {
        print("круг \(round): за 2 с окно экранным не стало")
    }
    usleep(600_000)
}
