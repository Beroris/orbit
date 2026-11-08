// Timing capture for the benchmark harness. Same line format as
// platforms/windows/src/metrics.rs. Timestamps are mach_absolute_time ticks, which are comparable
// across processes on one machine, so orbit-bench can line them up with its own clock.
// Without --bench-log this is one branch and a few counters.

import Darwin
import Foundation

func ticks() -> Int64 { Int64(mach_absolute_time()) }

let tickToMs: Double = {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    return Double(info.numer) / Double(info.denom) / 1_000_000
}()

struct ShowTimes {
    /// T0: hotkey callback entered.
    var t0: Int64 = 0
    /// T1: show requested, after the first frame was prepared.
    var t1: Int64 = 0
    /// T2: panel ordered front and first frame committed with CATransaction.flush.
    var t2: Int64 = 0
    /// T2b: the window server reported the panel visible (occlusion state changed).
    var t2Composed: Int64 = 0
    /// T3: panel is key and the field editor is first responder.
    var t3: Int64 = 0
}

final class Metrics {
    private let logPath: String?
    private var lines: [String] = []
    let benchMode: Bool
    /// Bounded summaries for the diagnostics page. The last 64 of each kind.
    private var showMs: [Float] = []
    private var queryMs: [Float] = []
    private(set) var showsTotal = 0
    private(set) var queriesTotal = 0
    private var pendingShow: (source: String, times: ShowTimes)?

    init(logPath: String?) {
        self.logPath = logPath
        benchMode = logPath != nil
    }

    private func pushBounded(_ list: inout [Float], _ value: Float) {
        if list.count == 64 { list.removeFirst() }
        list.append(value)
    }

    private func percentile(_ values: [Float], _ p: Float) -> Float {
        if values.isEmpty { return 0 }
        let sorted = values.sorted()
        return sorted[Int((Float(sorted.count - 1) * p).rounded())]
    }

    /// The line is held until the window server's visibility report fills in T2b, or until hide.
    func recordShow(source: String, times: ShowTimes) {
        showsTotal += 1
        // T3 stays 0 when the system didn't make the panel key at once. It may arrive later, below.
        if times.t3 != 0 { pushBounded(&showMs, Float(Double(times.t3 - times.t0) * tickToMs)) }
        if benchMode {
            commitPendingShow()
            pendingShow = (source, times)
        }
    }

    func panelBecameVisible() {
        if pendingShow != nil, pendingShow!.times.t2Composed == 0 {
            pendingShow!.times.t2Composed = ticks()
        }
    }

    func panelBecameKey() {
        if pendingShow != nil, pendingShow!.times.t3 == 0 {
            pendingShow!.times.t3 = ticks()
        }
    }

    func commitPendingShow() {
        guard let pending = pendingShow else { return }
        let t = pending.times
        lines.append("show,\(pending.source),\(t.t0),\(t.t1),\(t.t2),\(t.t2Composed),\(t.t3)")
        pendingShow = nil
    }

    /// T4 query received, T5 results built, T6 results drawn.
    func recordQuery(queryLength: Int, hits: Int, t4: Int64, t5: Int64, t6: Int64) {
        queriesTotal += 1
        pushBounded(&queryMs, Float(Double(t6 - t4) * tickToMs))
        if benchMode { lines.append("query,\(queryLength),\(hits),\(t4),\(t5),\(t6)") }
    }

    /// Enter-key received to action issued.
    func recordAction(kind: String, tEnter: Int64, tDispatched: Int64) {
        if benchMode { lines.append("action,\(kind),\(tEnter),\(tDispatched)") }
    }

    func recordHookDispatch(tHook: Int64, tReceived: Int64) {
        if benchMode { lines.append("hyper,\(tHook),\(tReceived)") }
    }

    /// macOS-only line. Bench lines can carry extra kinds: the Windows parser ignores unknown ones.
    func recordLine(_ line: String) {
        if benchMode { lines.append(line) }
    }

    var showSummary: (Float, Float) { (percentile(showMs, 0.5), percentile(showMs, 0.95)) }
    var querySummary: (Float, Float) { (percentile(queryMs, 0.5), percentile(queryMs, 0.95)) }

    func flush() {
        commitPendingShow()
        guard let logPath, !lines.isEmpty else { return }
        let text = lines.joined(separator: "\n") + "\n"
        lines.removeAll(keepingCapacity: true)
        if let file = fopen(logPath, "a") {
            fputs(text, file)
            fclose(file)
        }
    }
}

struct ProcessMemory {
    /// `phys_footprint` from task_info(TASK_VM_INFO): what Activity Monitor calls Memory.
    var footprint: UInt64 = 0
    /// `resident_size` from task_info(MACH_TASK_BASIC_INFO): includes shared framework pages.
    var resident: UInt64 = 0
    var peakFootprint: UInt64 = 0
}

func processMemory() -> ProcessMemory {
    var memory = ProcessMemory()
    var vm = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    let vmResult = withUnsafeMutablePointer(to: &vm) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
        }
    }
    if vmResult == KERN_SUCCESS {
        memory.footprint = vm.phys_footprint
        memory.peakFootprint = UInt64(vm.ledger_phys_footprint_peak)
    }
    var basic = mach_task_basic_info_data_t()
    var basicCount = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info_data_t>.size / MemoryLayout<natural_t>.size)
    let basicResult = withUnsafeMutablePointer(to: &basic) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(basicCount)) {
            task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &basicCount)
        }
    }
    if basicResult == KERN_SUCCESS { memory.resident = UInt64(basic.resident_size) }
    return memory
}

func formatBytes(_ bytes: UInt64) -> String {
    let value = Double(bytes)
    if value >= 1_048_576 { return String(format: "%.1f MB", value / 1_048_576) }
    return String(format: "%.0f KB", value / 1024)
}
