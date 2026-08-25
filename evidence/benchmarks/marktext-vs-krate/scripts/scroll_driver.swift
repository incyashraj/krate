import AppKit
import Foundation

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("ERROR: \(message)\n".utf8))
    exit(2)
}

let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 3,
      let pid = Int32(args[0]),
      let duration = Double(args[1]), duration > 0,
      let hz = Double(args[2]), hz > 0 else {
    fail("usage: scroll-driver PID DURATION_SECONDS EVENTS_PER_SECOND")
}

guard AXIsProcessTrusted() else {
    fail("Accessibility permission is required to post controlled scroll events")
}

let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
guard let rows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]],
      let row = rows.first(where: {
          ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid)
              && ($0[kCGWindowLayer as String] as? Int) == 0
      }),
      let boundsObject = row[kCGWindowBounds as String],
      let bounds = CGRect(dictionaryRepresentation: boundsObject as! CFDictionary) else {
    fail("no visible layer-0 window found for pid \(pid)")
}

NSRunningApplication(processIdentifier: pid)?.activate(options: [])
Thread.sleep(forTimeInterval: 0.5)

let source = CGEventSource(stateID: .hidSystemState)
let location = CGPoint(x: bounds.midX, y: bounds.midY)
let interval = 1.0 / hz
let expected = Int(duration * hz)
let started = DispatchTime.now().uptimeNanoseconds
var posted = 0

for index in 0..<expected {
    let direction: Int32 = (index / max(Int(hz * 2), 1)) % 2 == 0 ? -3 : 3
    guard let event = CGEvent(
        scrollWheelEvent2Source: source,
        units: .pixel,
        wheelCount: 1,
        wheel1: direction,
        wheel2: 0,
        wheel3: 0
    ) else { fail("could not construct scroll event") }
    event.location = location
    event.post(tap: .cghidEventTap)
    posted += 1
    let target = started + UInt64(Double(index + 1) * interval * 1_000_000_000)
    // One clock read per pass. Reading the clock again to compute `remaining`
    // raced the deadline: when it passed between the two reads, the unsigned
    // subtraction underflowed and the driver died with SIGTRAP mid-run.
    while true {
        let now = DispatchTime.now().uptimeNanoseconds
        if now >= target { break }
        let remaining = target - now
        if remaining > 1_000_000 {
            Thread.sleep(forTimeInterval: Double(remaining - 500_000) / 1_000_000_000)
        }
    }
}

let actual = Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000_000
let result: [String: Any] = [
    "schema": "krate.benchmark.scroll.v1",
    "pid": Int(pid),
    "requested_hz": hz,
    "posted_events": posted,
    "duration_seconds": actual,
    "actual_hz": Double(posted) / actual,
]
let encoded = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
FileHandle.standardOutput.write(encoded)
FileHandle.standardOutput.write(Data("\n".utf8))
