import Darwin

/// Сколько памяти держит процесс панели сейчас: резидентный размер из `task_info`.
///
/// Нужно одному вопросу этапа UI-4b — во что обходится `WKWebView`. Страница WebKit живёт в
/// своих процессах (`com.apple.WebKit.WebContent`), и их память этим числом не видна: замер
/// снимает её снаружи (`ps`), ADR 0046.
enum ProcessMemory {

    static func residentMegabytes() -> Double {
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
            }
        }
        guard result == KERN_SUCCESS else { return 0 }
        return Double(info.resident_size) / 1_048_576
    }
}
