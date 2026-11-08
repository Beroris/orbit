// App discovery. Scans the standard Applications folders on a background queue, caches the list
// to disk, and refreshes through FSEvents with a debounce. It never polls.

import AppKit
import COrbit
import CoreServices

struct AppRecord: Codable {
    var id: String
    var name: String
    var path: String
}

/// One more folder to index and watch. Lets a test add and remove an app without touching a real
/// Applications folder.
var extraApplicationFolder: String?

func applicationFolders() -> [String] {
    var folders = ["/Applications", "/System/Applications", "/System/Applications/Utilities", "/Applications/Utilities"]
    let home = NSHomeDirectory()
    folders.append("\(home)/Applications")
    if let extraApplicationFolder { folders.append(extraApplicationFolder) }
    return folders
}

/// Bundles worth finding that live outside the Applications folders.
private let extraBundles = ["/System/Library/CoreServices/Finder.app"]

private func record(forBundle path: String) -> AppRecord? {
    let plistPath = path + "/Contents/Info.plist"
    let info = NSDictionary(contentsOfFile: plistPath)
    // Background-only agents and helpers are not something to launch from a launcher.
    if let info, (info["LSBackgroundOnly"] as? Bool == true) { return nil }
    let name = FileManager.default.displayName(atPath: path)
    let clean = name.hasSuffix(".app") ? String(name.dropLast(4)) : name
    let id = (info?["CFBundleIdentifier"] as? String) ?? path
    return AppRecord(id: id, name: clean, path: path)
}

/// Shallow by design: bundles in each folder and one level of vendor subfolders. It does not
/// descend into bundles, so it stays fast and cannot wander over the disk.
func scanApplications() -> [AppRecord] {
    let fm = FileManager.default
    var seenIds = Set<String>()
    var seenPaths = Set<String>()
    var out: [AppRecord] = []
    func add(_ path: String) {
        guard seenPaths.insert(path).inserted, let app = record(forBundle: path) else { return }
        if seenIds.insert(app.id).inserted { out.append(app) }
    }
    for folder in applicationFolders() {
        guard let entries = try? fm.contentsOfDirectory(atPath: folder) else { continue }
        for entry in entries where !entry.hasPrefix(".") {
            let path = folder + "/" + entry
            if entry.hasSuffix(".app") {
                add(path)
            } else if let inner = try? fm.contentsOfDirectory(atPath: path) {
                for sub in inner where sub.hasSuffix(".app") { add(path + "/" + sub) }
            }
        }
    }
    extraBundles.forEach(add)
    out.sort { $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending }
    return out
}

func encodeApps(_ apps: [AppRecord]) -> String {
    guard let data = try? JSONEncoder().encode(apps) else { return "[]" }
    return String(data: data, encoding: .utf8) ?? "[]"
}

/// Watches the folders and calls `onDirty` on the main queue after they go quiet.
final class AppWatcher {
    /// Installers touch many files. Wait for the burst to finish before reindexing.
    private static let debounceSeconds: CFTimeInterval = 5
    private var stream: FSEventStreamRef?
    private let onDirty: () -> Void
    private var pending: DispatchWorkItem?
    private let roots: [String]

    init(onDirty: @escaping () -> Void) {
        self.onDirty = onDirty
        roots = applicationFolders().filter { FileManager.default.fileExists(atPath: $0) }
    }

    func start() {
        guard stream == nil, !roots.isEmpty else { return }
        var context = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(), retain: nil, release: nil, copyDescription: nil)
        let callback: FSEventStreamCallback = { _, info, count, paths, _, _ in
            guard let info else { return }
            let watcher = Unmanaged<AppWatcher>.fromOpaque(info).takeUnretainedValue()
            let list = unsafeBitCast(paths, to: NSArray.self) as? [String] ?? []
            watcher.received(list.prefix(count))
        }
        let flags = FSEventStreamCreateFlags(kFSEventStreamCreateFlagUseCFTypes | kFSEventStreamCreateFlagIgnoreSelf)
        guard let created = FSEventStreamCreate(nil, callback, &context, roots as CFArray, FSEventStreamEventId(kFSEventStreamEventIdSinceNow), AppWatcher.debounceSeconds, flags) else { return }
        FSEventStreamSetDispatchQueue(created, .main)
        FSEventStreamStart(created)
        stream = created
    }

    func stop() {
        guard let stream else { return }
        FSEventStreamStop(stream)
        FSEventStreamInvalidate(stream)
        FSEventStreamRelease(stream)
        self.stream = nil
        pending?.cancel()
    }

    /// Apps rewrite files inside their own bundles while running, so a change deep inside a bundle
    /// is ignored. A folder that holds bundles, or the bundle itself, is what matters.
    static func isRelevant(_ path: String, roots: [String]) -> Bool {
        let trimmed = path.hasSuffix("/") ? String(path.dropLast()) : path
        for root in roots {
            guard trimmed == root || trimmed.hasPrefix(root + "/") else { continue }
            if trimmed == root { return true }
            let rest = trimmed.dropFirst(root.count + 1).split(separator: "/")
            // Allowed: X.app, Vendor, Vendor/X.app. Anything below a bundle isn't.
            if rest.count == 1 { return true }
            if rest.count == 2, !rest[0].hasSuffix(".app"), rest[1].hasSuffix(".app") { return true }
        }
        return false
    }

    private func received(_ paths: ArraySlice<String>) {
        guard paths.contains(where: { AppWatcher.isRelevant($0, roots: roots) }) else { return }
        pending?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.onDirty() }
        pending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + AppWatcher.debounceSeconds, execute: work)
    }
}

/// App icons, small and bounded. The first request decodes through NSWorkspace off the main thread
/// and stores raw pixels on disk. Later requests read those bytes, without loading IconServices
/// or the app's full-size icon family into this process.
final class IconCache {
    static let pixelSize = 56 // 28 points at 2x

    private var images: [String: CGImage] = [:]
    private var order: [String] = []
    private var inFlight = Set<String>()
    private(set) var capacity: Int
    private let directory: String
    private let queue = DispatchQueue(label: "orbit.icons", qos: .utility)
    var onLoaded: (() -> Void)?

    init(capacity: Int, dataDir: String) {
        self.capacity = capacity
        directory = dataDir + "/icons"
    }

    var count: Int { images.count }
    var bytes: Int { images.count * IconCache.pixelSize * IconCache.pixelSize * 4 }

    func setCapacity(_ value: Int) {
        capacity = value
        evict()
    }

    func clear() {
        images.removeAll()
        order.removeAll()
    }

    private func evict() {
        while order.count > capacity {
            images[order.removeFirst()] = nil
        }
    }

    /// Returns the icon if it's ready, and starts loading it if not. Never blocks.
    func image(path: String) -> CGImage? {
        if let image = images[path] {
            if order.last != path, let at = order.firstIndex(of: path) {
                order.remove(at: at)
                order.append(path)
            }
            return image
        }
        guard inFlight.insert(path).inserted else { return nil }
        let directory = directory
        queue.async { [weak self] in
            let loaded = autoreleasepool { IconCache.load(path: path, directory: directory) }
            DispatchQueue.main.async {
                guard let self else { return }
                self.inFlight.remove(path)
                guard let loaded else { return }
                self.images[path] = loaded
                self.order.append(path)
                self.evict()
                self.onLoaded?()
            }
        }
        return nil
    }

    private static func cachePath(path: String, directory: String) -> String {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        let modified = (try? FileManager.default.attributesOfItem(atPath: path)[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
        for byte in "\(path)|\(Int(modified))".utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01B3
        }
        return directory + "/" + String(hash, radix: 16) + ".bgra"
    }

    /// Removes cached icons that no indexed app's current path and modified time lead to, such as
    /// those of removed or updated apps. Runs after each app scan, off the main thread.
    static func pruneStale(directory: String, appPaths: [String]) {
        guard let files = try? FileManager.default.contentsOfDirectory(atPath: directory) else { return }
        let current = Set(appPaths.map { (cachePath(path: $0, directory: directory) as NSString).lastPathComponent })
        for file in files where file.hasSuffix(".bgra") && !current.contains(file) {
            try? FileManager.default.removeItem(atPath: directory + "/" + file)
        }
    }

    private static func load(path: String, directory: String) -> CGImage? {
        let size = pixelSize
        let byteCount = size * size * 4
        let cache = cachePath(path: path, directory: directory)
        if let data = FileManager.default.contents(atPath: cache), data.count == byteCount {
            return makeImage(data, size: size)
        }
        guard FileManager.default.fileExists(atPath: path) else { return nil }
        let icon = NSWorkspace.shared.icon(forFile: path)
        var pixels = Data(count: byteCount)
        let drawn: Bool = pixels.withUnsafeMutableBytes { raw in
            guard let context = CGContext(data: raw.baseAddress, width: size, height: size, bitsPerComponent: 8, bytesPerRow: size * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue) else { return false }
            context.interpolationQuality = .high
            let graphics = NSGraphicsContext(cgContext: context, flipped: false)
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = graphics
            icon.draw(in: NSRect(x: 0, y: 0, width: size, height: size))
            NSGraphicsContext.restoreGraphicsState()
            return true
        }
        guard drawn else { return nil }
        try? FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: cache, contents: pixels)
        return makeImage(pixels, size: size)
    }

    private static func makeImage(_ pixels: Data, size: Int) -> CGImage? {
        guard let provider = CGDataProvider(data: pixels as CFData) else { return nil }
        return CGImage(width: size, height: size, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: size * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue), provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent)
    }
}
