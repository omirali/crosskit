import SwiftUI
import WebKit

/// Встроенный браузер: адрес, размер окна, консоль, снимок и выбор элемента — агенту.
struct BrowserPane: View {

    @Bindable var browser: BrowserModel

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                Button("Назад", systemImage: "chevron.left") { browser.back() }.disabled(!browser.canGoBack)
                Button("Вперёд", systemImage: "chevron.right") { browser.forward() }.disabled(!browser.canGoForward)
                Button("Перезагрузить", systemImage: "arrow.clockwise") { browser.reload() }
                TextField("Адрес", text: $browser.address)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { browser.go() }
                Picker("Размер окна", selection: $browser.device) {
                    ForEach(BrowserModel.Device.allCases) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                Toggle(isOn: Binding(get: { browser.isPicking }, set: { _ in browser.togglePicking() })) {
                    Label("Указать элемент", systemImage: "cursorarrow.rays")
                }
                .toggleStyle(.button)
                .help("Нажмите элемент на странице — его селектор попадёт в сообщение агенту")
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .padding(.horizontal, 10).padding(.vertical, 7)
            .background(.background)
            if browser.isPicking {
                Text("Режим выбора: нажмите на элемент страницы — он попадёт в сообщение агенту.")
                    .font(.caption).foregroundStyle(.tint)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12).padding(.vertical, 5)
                    .background(.tint.opacity(0.08))
            }
            Divider()
            WebView(view: browser.view)
                .frame(maxWidth: browser.device.width ?? .infinity)
                .clipShape(.rect(cornerRadius: browser.device == .desktop ? 0 : 18))
                .padding(browser.device == .desktop ? 0 : 16)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(.background.secondary)
            if browser.consoleOpen {
                Divider()
                console.frame(height: 170)
            }
            Divider()
            HStack(spacing: 8) {
                Text(browser.status).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Spacer()
                Toggle(isOn: $browser.consoleOpen) {
                    Text(browser.errorCount > 0 ? "Консоль · \(browser.errorCount)" : "Консоль")
                }
                .toggleStyle(.button)
                Button("Снимок → агенту") { browser.snapshot() }.buttonStyle(.borderedProminent)
                Button("Открыть в Safari") { browser.openInSafari() }
            }
            .controlSize(.small)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .background(.background)
        }
    }

    private var console: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(browser.console) { line in
                    HStack(spacing: 8) {
                        Text(line.level == .error ? "✕" : line.level == .warn ? "!" : "i").frame(width: 14)
                        Text(line.text).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                        if line.level == .error {
                            Button("Агенту") { browser.send(line) }.controlSize(.mini).buttonStyle(.borderedProminent)
                        }
                    }
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(line.level == .error ? .red : line.level == .warn ? .orange : .secondary)
                    .padding(.horizontal, 12).padding(.vertical, 3)
                    .background(line.level == .error ? Color.red.opacity(0.06) : line.level == .warn ? Color.orange.opacity(0.07) : .clear)
                }
                if browser.console.isEmpty {
                    Text("Консоль пуста").font(.caption).foregroundStyle(.secondary).padding(12)
                }
            }
        }
        .background(.background.secondary)
    }
}

/// `WKWebView` живёт в модели браузера: вкладка, которую закрыли и открыли, не теряет страницу.
private struct WebView: NSViewRepresentable {
    let view: WKWebView

    func makeNSView(context: Context) -> WKWebView { view }
    func updateNSView(_ nsView: WKWebView, context: Context) {}
}
