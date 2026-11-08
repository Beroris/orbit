// The launcher window: a non-activating panel that can take keys, a native text field, and one
// custom-drawn view for rows and footer. Its height is fixed, so async results can't move it.

import AppKit
import COrbit
import IOSurface

enum Layout {
    static let width: CGFloat = 680
    static let inputHeight: CGFloat = 56
    static let rowHeight: CGFloat = 44
    static let footerHeight: CGFloat = 32
    static let padding: CGFloat = 8
    static let icon: CGFloat = 28

    static func height(rows: Int) -> CGFloat { inputHeight + padding * 2 + rowHeight * CGFloat(rows) + footerHeight }
}

struct Theme {
    var background, text, secondary, selection, selectionText, divider, accent, footer: NSColor

    private static func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat) -> NSColor {
        NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: 1)
    }

    static let dark = Theme(background: rgb(30, 30, 33), text: rgb(238, 238, 240), secondary: rgb(152, 152, 160), selection: rgb(58, 58, 65), selectionText: rgb(255, 255, 255), divider: rgb(56, 56, 62), accent: rgb(245, 158, 11), footer: rgb(25, 25, 28))
    static let light = Theme(background: rgb(250, 250, 251), text: rgb(26, 26, 29), secondary: rgb(104, 104, 113), selection: rgb(228, 228, 233), selectionText: rgb(16, 16, 18), divider: rgb(226, 226, 231), accent: rgb(180, 83, 9), footer: rgb(243, 243, 245))

    static func resolve(appearance: String) -> Theme {
        switch appearance {
        case "dark": return .dark
        case "light": return .light
        default:
            let name = NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua])
            return name == .darkAqua ? .dark : .light
        }
    }
}

/// `.nonactivatingPanel` means the previous app stays active while this window holds the keyboard,
/// so showing costs no app switch and hiding needs no focus juggling.
final class LauncherPanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    var onKeyEquivalent: ((NSEvent) -> Bool)?

    init(rows: Int) {
        super.init(contentRect: NSRect(x: 0, y: 0, width: Layout.width, height: Layout.height(rows: rows)), styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        isFloatingPanel = true
        level = .modalPanel
        hidesOnDeactivate = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        isReleasedWhenClosed = false
        animationBehavior = .none
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]
        isMovable = false
    }

    /// An inactive app's menu bar never sees key equivalents, so ⌘K, ⌘↵ and editing keys are
    /// handled here.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if onKeyEquivalent?(event) == true { return true }
        return super.performKeyEquivalent(with: event)
    }
}

/// Draws the whole panel background, rows and footer in one pass. There is no retained row view
/// tree to update, so a frame stays cheap however many results change.
///
/// A frame is drawn on the CPU into a new IOSurface, which then becomes the layer's contents. When
/// AppKit's own backing store was used, replacing visible rows made the process allocate about 120 MB
/// of graphics memory, charged to its footprint for about 1.2 s after each frame. Typing kept it
/// allocated. Drawing on the CPU needs none of it, and an IOSurface reaches the window server without
/// the copy a CGImage needs. See platforms/macos/README.md.
final class ResultsView: NSView {
    unowned let controller: AppController
    var theme = Theme.dark

    private lazy var titleFont = NSFont.systemFont(ofSize: 15, weight: .semibold)
    private lazy var smallFont = NSFont.systemFont(ofSize: 13)
    private lazy var hintFont = NSFont.systemFont(ofSize: 12)
    private struct GlyphKey: Hashable {
        var glyph: Int
        var color: NSColor
    }
    private var glyphCache: [GlyphKey: CGImage] = [:]
    private let paragraph: NSParagraphStyle = {
        let style = NSMutableParagraphStyle()
        style.lineBreakMode = .byTruncatingTail
        return style
    }()

    init(controller: AppController, frame: NSRect) {
        self.controller = controller
        super.init(frame: frame)
        wantsLayer = true
        // The layer rounds the corners, which costs far less than clipping every fill on the CPU.
        layer?.cornerRadius = 12
        layer?.masksToBounds = true
        let tracking = NSTrackingArea(rect: .zero, options: [.mouseMoved, .activeAlways, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(tracking)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    private static func symbolName(_ glyph: Int) -> String {
        switch Int(glyph) {
        case ORBIT_GLYPH_SEARCH: return "magnifyingglass"
        case ORBIT_GLYPH_NOTE: return "note.text"
        case ORBIT_GLYPH_SNIPPET: return "text.quote"
        case ORBIT_GLYPH_CLIPBOARD: return "doc.on.clipboard"
        case ORBIT_GLYPH_CALC: return "equal.square"
        case ORBIT_GLYPH_LINK: return "link"
        case ORBIT_GLYPH_WEB: return "globe"
        case ORBIT_GLYPH_PIN: return "pin"
        case ORBIT_GLYPH_FOLDER: return "folder"
        case ORBIT_GLYPH_COMMAND: return "command"
        case ORBIT_GLYPH_INFO: return "info.circle"
        case ORBIT_GLYPH_IMAGE: return "photo"
        case ORBIT_GLYPH_SETTINGS: return "gearshape"
        case ORBIT_GLYPH_DIAGNOSTICS: return "gauge.with.dots.needle.33percent"
        case ORBIT_GLYPH_RELOAD: return "arrow.clockwise"
        case ORBIT_GLYPH_PAUSE: return "pause.circle"
        case ORBIT_GLYPH_POWER: return "power"
        case ORBIT_GLYPH_LOCK: return "lock"
        case ORBIT_GLYPH_MOON: return "moon"
        case ORBIT_GLYPH_DELETE: return "trash"
        case ORBIT_GLYPH_HELP: return "questionmark.circle"
        case ORBIT_GLYPH_TERMINAL: return "terminal"
        default: return "circle"
        }
    }

    /// Rasterises a symbol once, in one colour, into a small bitmap. This cache costs under 10 KB an
    /// entry and is bounded by glyph count times the few colours a theme uses.
    private func glyphImage(_ glyph: Int, color: NSColor) -> CGImage? {
        let key = GlyphKey(glyph: glyph, color: color)
        if let cached = glyphCache[key] { return cached }
        let size = 44 // 22 points at 2x
        let configuration = NSImage.SymbolConfiguration(pointSize: 15, weight: .regular)
        guard let image = NSImage(systemSymbolName: ResultsView.symbolName(glyph), accessibilityDescription: nil)?.withSymbolConfiguration(configuration),
              let context = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue) else { return nil }
        context.scaleBy(x: 2, y: 2)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: false)
        let symbol = image.size
        let rect = NSRect(x: (22 - symbol.width) / 2, y: (22 - symbol.height) / 2, width: symbol.width, height: symbol.height)
        image.draw(in: rect)
        color.setFill()
        NSRect(x: 0, y: 0, width: 22, height: 22).fill(using: .sourceIn)
        NSGraphicsContext.restoreGraphicsState()
        let result = context.makeImage()
        glyphCache[key] = result
        return result
    }

    private func drawGlyph(_ glyph: Int, in slot: NSRect, color: NSColor) {
        guard let image = glyphImage(glyph, color: color), let context = NSGraphicsContext.current?.cgContext else { return }
        let rect = CGRect(x: slot.midX - 11, y: slot.midY - 11, width: 22, height: 22)
        context.saveGState()
        context.translateBy(x: rect.minX, y: rect.maxY)
        context.scaleBy(x: 1, y: -1)
        context.draw(image, in: CGRect(x: 0, y: 0, width: 22, height: 22))
        context.restoreGState()
    }

    private func text(_ string: String, font: NSFont, color: NSColor, in rect: NSRect) {
        let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .paragraphStyle: paragraph]
        let height = font.ascender - font.descender + font.leading
        let y = rect.midY - height / 2
        (string as NSString).draw(with: NSRect(x: rect.minX, y: y, width: max(0, rect.width), height: height + 2), options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine], attributes: attributes)
    }

    private func width(_ string: String, font: NSFont) -> CGFloat {
        (string as NSString).size(withAttributes: [.font: font]).width
    }

    override var wantsUpdateLayer: Bool { true }

    override func updateLayer() {
        if controller.isLauncherVisible { renderFrame() }
    }

    override func viewDidChangeBackingProperties() {
        super.viewDidChangeBackingProperties()
        needsDisplay = true
    }

    /// Draws a frame and hands it to the layer. It can run before the window is on screen, so the
    /// first thing the window server shows is the finished frame.
    func renderFrame() {
        let scale = window?.backingScaleFactor ?? 2
        let width = Int((bounds.width * scale).rounded(.up))
        let height = Int((bounds.height * scale).rounded(.up))
        let properties: [IOSurfacePropertyKey: Any] = [.width: width, .height: height, .bytesPerElement: 4, .pixelFormat: 0x42475241]
        guard let space = CGColorSpace(name: CGColorSpace.sRGB), let surface = IOSurface(properties: properties) else { return }
        surface.setAttachment(space.copyPropertyList() as Any, forKey: kIOSurfaceColorSpace as String)
        surface.lock(options: [], seed: nil)
        defer { surface.unlock(options: [], seed: nil) }
        guard let context = CGContext(data: surface.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: surface.bytesPerRow, space: space, bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue) else { return }
        context.clear(CGRect(x: 0, y: 0, width: width, height: height))
        context.translateBy(x: 0, y: CGFloat(height))
        context.scaleBy(x: scale, y: -scale)
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(cgContext: context, flipped: true)
        draw(bounds)
        NSGraphicsContext.restoreGraphicsState()
        layer?.contentsScale = scale
        layer?.contents = surface
    }

    /// Lets go of the last frame, about 5 MB at 2x, while the panel is hidden.
    func releaseFrame() {
        layer?.contents = nil
    }

    override func draw(_ dirtyRect: NSRect) {
        let view = controller.view
        let bounds = self.bounds
        defer {
            theme.divider.setStroke()
            let edge = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 11.5, yRadius: 11.5)
            edge.lineWidth = 1
            edge.stroke()
        }
        theme.background.setFill()
        bounds.fill()

        drawGlyph(view.scope.inputGlyph, in: NSRect(x: Layout.padding + 10, y: 0, width: Layout.icon, height: Layout.inputHeight), color: theme.secondary)
        theme.divider.setFill()
        NSRect(x: 0, y: Layout.inputHeight - 1, width: bounds.width, height: 1).fill()

        let rowX = Layout.padding
        let rowWidth = bounds.width - 2 * rowX
        let iconX = rowX + 10
        let textX = iconX + Layout.icon + 12
        let rightEdge = rowX + rowWidth - 12
        let listTop = Layout.inputHeight + Layout.padding

        if view.rows.isEmpty, !controller.waitingForData, let message = view.scope.emptyMessage {
            text(message, font: smallFont, color: theme.secondary, in: NSRect(x: textX, y: listTop, width: rightEdge - textX, height: Layout.rowHeight))
        }

        let first = view.scroll
        let last = min(first + view.visibleRows, view.rows.count)
        var index = first
        while index < last {
            let row = view.rows[index]
            let y = listTop + CGFloat(index - first) * Layout.rowHeight
            let selected = index == view.selected
            if selected {
                theme.selection.setFill()
                NSBezierPath(roundedRect: NSRect(x: rowX, y: y + 1, width: rowWidth, height: Layout.rowHeight - 2), xRadius: 8, yRadius: 8).fill()
            }
            let titleColor = selected ? theme.selectionText : theme.text
            let iconRect = NSRect(x: iconX, y: y + (Layout.rowHeight - Layout.icon) / 2, width: Layout.icon, height: Layout.icon)
            switch row.icon {
            case .app(let path):
                if let image = controller.icons.image(path: path), let context = NSGraphicsContext.current?.cgContext {
                    context.saveGState()
                    context.translateBy(x: iconRect.minX, y: iconRect.maxY)
                    context.scaleBy(x: 1, y: -1)
                    context.draw(image, in: CGRect(x: 0, y: 0, width: iconRect.width, height: iconRect.height))
                    context.restoreGState()
                }
            case .glyph(let glyph):
                drawGlyph(glyph, in: iconRect, color: row.accent ? theme.accent : theme.secondary)
            }

            let kindWidth = row.kind.isEmpty ? 0 : ceil(width(row.kind, font: hintFont))
            let textRight = rightEdge - kindWidth - (kindWidth > 0 ? 12 : 0)
            let titleWidth = ceil(width(row.title, font: titleFont))
            let titleLimit = row.subtitle.isEmpty ? textRight - textX : (textRight - textX) * 0.7
            let titleRight = min(textX + titleWidth + 2, textX + titleLimit)
            text(row.title, font: titleFont, color: titleColor, in: NSRect(x: textX, y: y, width: titleRight - textX, height: Layout.rowHeight))
            if !row.subtitle.isEmpty {
                let subtitleX = titleRight + 10
                if subtitleX + 24 < textRight {
                    text(row.subtitle, font: smallFont, color: theme.secondary, in: NSRect(x: subtitleX, y: y, width: textRight - subtitleX, height: Layout.rowHeight))
                }
            }
            if kindWidth > 0 {
                text(row.kind, font: hintFont, color: theme.secondary, in: NSRect(x: rightEdge - kindWidth, y: y, width: kindWidth + 1, height: Layout.rowHeight))
            }
            index += 1
        }

        // A small scrollbar, only when there is more than fits.
        let total = view.rows.count
        if total > view.visibleRows {
            let track = CGFloat(view.visibleRows) * Layout.rowHeight
            let thumb = max(16, track * CGFloat(view.visibleRows) / CGFloat(total))
            let thumbY = listTop + (track - thumb) * CGFloat(first) / CGFloat(total - view.visibleRows)
            theme.divider.setFill()
            NSBezierPath(roundedRect: NSRect(x: bounds.width - 5, y: thumbY, width: 3, height: thumb), xRadius: 1.5, yRadius: 1.5).fill()
        }

        // Footer: status on the left, what keys do on the right.
        let footerTop = bounds.height - Layout.footerHeight
        theme.footer.setFill()
        NSRect(x: 0, y: footerTop, width: bounds.width, height: Layout.footerHeight).fill()
        theme.divider.setFill()
        NSRect(x: 0, y: footerTop, width: bounds.width, height: 1).fill()
        let hints = view.footerHints()
        let hintsWidth = ceil(width(hints, font: hintFont))
        let hintLeft = bounds.width - 14 - hintsWidth
        text(hints, font: hintFont, color: theme.secondary, in: NSRect(x: hintLeft, y: footerTop, width: hintsWidth + 1, height: Layout.footerHeight))
        let status = controller.status ?? view.scope.label
        text(status, font: hintFont, color: controller.status != nil ? theme.accent : theme.secondary, in: NSRect(x: 16, y: footerTop, width: hintLeft - 28, height: Layout.footerHeight))
    }

    /// Which row is under a y, if any.
    private func rowIndex(atY y: CGFloat) -> Int? {
        let view = controller.view
        let top = Layout.inputHeight + Layout.padding
        guard y >= top else { return nil }
        let visible = min(view.visibleRows, max(0, view.rows.count - view.scroll))
        let index = Int((y - top) / Layout.rowHeight)
        return index < visible ? index + view.scroll : nil
    }

    private var lastMouse: NSPoint?
    private var scrollRemainder: CGFloat = 0

    override func mouseMoved(with event: NSEvent) {
        // A window appearing under a still pointer must not steal the keyboard selection.
        let point = convert(event.locationInWindow, from: nil)
        defer { lastMouse = point }
        guard let last = lastMouse, last != point, let index = rowIndex(atY: point.y) else { return }
        controller.mouseSelect(index)
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let index = rowIndex(atY: point.y) {
            controller.mouseClick(index, secondary: false)
        } else {
            controller.focusField()
        }
    }

    override func rightMouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let index = rowIndex(atY: point.y) { controller.mouseClick(index, secondary: true) }
    }

    override func scrollWheel(with event: NSEvent) {
        // Trackpads send many small deltas, and a wheel sends a few large ones.
        scrollRemainder -= event.scrollingDeltaY / (event.hasPreciseScrollingDeltas ? Layout.rowHeight : 1)
        let rows = Int(scrollRemainder)
        guard rows != 0 else { return }
        scrollRemainder -= CGFloat(rows)
        controller.scroll(rows)
    }
}
