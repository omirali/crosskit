import AppKit
import WebKit

/// Снимок окна панели её же средствами (`--snapshot=<файл>`), без разрешения «Запись экрана».
///
/// AppKit рисует своё содержимое в картинку (`cacheDisplay`), а страницу графа — нет: у
/// `WKWebView` слой живёт в процессе WebKit, и его отдаёт `takeSnapshot`. Снимок — их наложение
/// на сплошной фон окна этой темы. Размытия материала в нём нет: его делает оконный сервер, а
/// не процесс; это единственное отличие от экрана, и оно сказано здесь и в README.
enum WindowSnapshot {

    static func write(_ content: NSView, webView: WKWebView?, to path: String, done: @escaping (Bool) -> Void) {
        let bounds = content.bounds
        guard let window = content.bitmapImageRepForCachingDisplay(in: bounds) else { return done(false) }
        content.cacheDisplay(in: bounds, to: window)

        let finish: (NSImage?) -> Void = { page in
            let scale = CGFloat(window.pixelsWide) / max(bounds.width, 1)
            guard let output = NSBitmapImageRep(
                bitmapDataPlanes: nil, pixelsWide: window.pixelsWide, pixelsHigh: window.pixelsHigh,
                bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
                let context = NSGraphicsContext(bitmapImageRep: output) else { return done(false) }
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = context
            context.cgContext.scaleBy(x: scale, y: scale)
            content.effectiveAppearance.performAsCurrentDrawingAppearance {
                NSColor.windowBackgroundColor.setFill()
                NSBezierPath(roundedRect: bounds, xRadius: Tokens.windowRadius, yRadius: Tokens.windowRadius).fill()
            }
            window.draw(in: bounds)
            if let page, let webView, webView.superview != nil {
                // Содержимое перевёрнуто, а картинка рисуется снизу вверх.
                let frame = webView.convert(webView.bounds, to: content)
                page.draw(in: NSRect(x: frame.minX, y: bounds.height - frame.maxY,
                                     width: frame.width, height: frame.height))
            }
            NSGraphicsContext.restoreGraphicsState()
            let written = output.representation(using: .png, properties: [:])
                .map { (try? $0.write(to: URL(fileURLWithPath: path))) != nil } ?? false
            done(written)
        }
        guard let webView, webView.superview != nil else { return finish(nil) }
        webView.takeSnapshot(with: nil) { image, _ in finish(image) }
    }
}
