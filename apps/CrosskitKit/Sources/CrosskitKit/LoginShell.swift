import Foundation

/// Запуск программы так, как её запустил бы терминал владельца.
///
/// У приложения из Finder PATH — только системный, а `claude` установлен в `/opt/homebrew/bin`
/// или `~/.local/bin`. Login-оболочка (`zsh -l`) читает `~/.zprofile` и даёт тот же PATH, что в
/// терминале. `"$@"` передаёт аргументы как есть: ни имя задачи, ни текст не становятся командой.
enum LoginShell {

    static let shell = URL(fileURLWithPath: "/bin/zsh")

    /// Аргументы `zsh`, которые запускают `program` с `arguments`.
    static func arguments(running program: String, with arguments: [String]) -> [String] {
        ["-l", "-c", "exec \"$@\"", "crosskit", program] + arguments
    }

    /// Процесс, готовый к запуску: программа через login-оболочку, в каталоге `directory`.
    static func process(running program: String, with arguments: [String], in directory: URL? = nil) -> Process {
        let process = Process()
        process.executableURL = shell
        process.arguments = Self.arguments(running: program, with: arguments)
        if let directory { process.currentDirectoryURL = directory }
        return process
    }
}
