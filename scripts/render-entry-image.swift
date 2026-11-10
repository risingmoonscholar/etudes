// Render one local HTML page offscreen with WebKit and write a JPEG.
// Used by scripts/render-entry-image. No window is shown and no app is driven:
// the web view lives in a borderless window placed off every screen.
//
//   render-entry-image-bin PAGE.html SITE_ROOT WIDTH OUT.jpg [SELECTOR]
//
// The page is laid out at WIDTH CSS pixels, as tall as its content. With a
// SELECTOR, only that element (plus 16 px around it) is written; without one,
// the whole page. Three pixels per CSS pixel, and always the light theme, so
// the picture does not depend on the machine's screen or appearance setting.
import AppKit
import WebKit

let args = CommandLine.arguments
guard args.count >= 5, let parsedWidth = Double(args[3]), parsedWidth > 0 else {
    FileHandle.standardError.write("usage: render-entry-image-bin PAGE.html SITE_ROOT WIDTH OUT.jpg [SELECTOR]\n".data(using: .utf8)!)
    exit(2)
}
let width: Double = parsedWidth
let page = URL(fileURLWithPath: args[1])
let root = URL(fileURLWithPath: args[2], isDirectory: true)
let out = URL(fileURLWithPath: args[4])
let selector = args.count > 5 ? args[5] : ""
let margin = 16.0
let scale = 3.0

func fail(_ message: String, _ code: Int32 = 1) -> Never {
    FileHandle.standardError.write("FAIL \(message)\n".data(using: .utf8)!)
    exit(code)
}

final class Renderer: NSObject, WKNavigationDelegate {
    let view: WKWebView
    let window: NSWindow

    init(width: Double) {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        let frame = NSRect(x: 0, y: 0, width: width, height: 800)
        view = WKWebView(frame: frame, configuration: config)
        window = NSWindow(contentRect: NSRect(x: -20000, y: -20000, width: width, height: 800),
                          styleMask: [.borderless], backing: .buffered, defer: false)
        super.init()
        window.contentView = view
        view.navigationDelegate = self
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // Wait for the fonts, then size the view to the whole page.
        view.evaluateJavaScript("document.fonts.ready.then(() => document.documentElement.scrollHeight)") { _, _ in
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { self.resize() }
        }
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        fail("the page did not load: \(error)")
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        fail("the page did not load: \(error)")
    }

    func resize() {
        view.evaluateJavaScript("document.documentElement.scrollHeight") { result, _ in
            guard let height = (result as? NSNumber)?.doubleValue, height > 0 else { fail("the page has no height") }
            let size = NSSize(width: width, height: height)
            self.window.setContentSize(size)
            self.view.frame = NSRect(origin: .zero, size: size)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { self.shoot(pageHeight: height) }
        }
    }

    func shoot(pageHeight: Double) {
        let measure = selector.isEmpty ? "null" : """
            (() => { const e = document.querySelector(\(String(reflecting: selector)));
              if (!e) return null; const r = e.getBoundingClientRect();
              return [r.left + scrollX, r.top + scrollY, r.width, r.height]; })()
            """
        view.evaluateJavaScript(measure) { result, _ in
            var rect = NSRect(x: 0, y: 0, width: width, height: pageHeight)
            if !selector.isEmpty {
                guard let r = result as? [NSNumber], r.count == 4 else { fail("no element matches \(selector)") }
                let x = max(0, r[0].doubleValue - margin), y = max(0, r[1].doubleValue - margin)
                rect = NSRect(x: x, y: y,
                              width: min(width - x, r[2].doubleValue + 2 * margin),
                              height: min(pageHeight - y, r[3].doubleValue + 2 * margin))
            }
            let config = WKSnapshotConfiguration()
            config.rect = rect
            // Three pixels per CSS pixel, as on a phone screen.
            config.snapshotWidth = NSNumber(value: rect.width * scale)
            self.view.takeSnapshot(with: config) { image, error in
                guard let image = image, let tiff = image.tiffRepresentation,
                      let rep = NSBitmapImageRep(data: tiff),
                      let jpg = rep.representation(using: .jpeg, properties: [.compressionFactor: 0.9]) else {
                    fail("the snapshot failed: \(String(describing: error))")
                }
                do { try jpg.write(to: out) } catch { fail("cannot write \(out.path): \(error)") }
                print("ok   wrote \(out.path) (\(rep.pixelsWide)x\(rep.pixelsHigh) pixels, \(Int(rect.width)) CSS px wide)")
                exit(0)
            }
        }
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
app.appearance = NSAppearance(named: .aqua)
let renderer = Renderer(width: width)
renderer.view.loadFileURL(page, allowingReadAccessTo: root)
DispatchQueue.main.asyncAfter(deadline: .now() + 60) { fail("timed out after 60 s", 2) }
app.run()
