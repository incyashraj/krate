// interact: the interaction latencies of IC-999, timed from outside.
//
// Input is posted as CGEvents (needs Accessibility for Terminal); the app's
// window is recorded with ScreenCaptureKit (needs Screen Recording), which
// hands each changed frame with the time it was displayed. A latency is
// the display time of the first frame that shows the answer minus the time
// the input event was posted, both on the same mach clock.
//
//   interact typing   --pid P [--note NAME] [--keys 30]
//   interact switcher --pid P --query Q --expect NAME [--phrases FILE]
//   interact search   --pid P --query Q --expect NAME [--seconds 8]
//   interact scroll   --pid P [--seconds 20] [--hz 125]
//   interact --self-test        the decision logic, on synthetic frames
//
// Before every event it checks that the frontmost app is the one under
// test and stops (exit 4, "focus_lost") rather than type into another app.
// Exit 3: a permission is missing; the message says which.

import AppKit
import CoreMedia
import Foundation
import ScreenCaptureKit
import VideoToolbox

// MARK: decision logic (pure; exercised by --self-test)

struct Frame {
    let tNs: UInt64
    let hash: UInt64
    var image: CGImage?
}

/// The first frame after `afterNs` whose picture was not seen while
/// learning: caret blink states and any other steady animation are in
/// `seen`, so only the input's echo counts.
func firstNewFrame(_ frames: [Frame], after afterNs: UInt64, seen: Set<UInt64>) -> Frame? {
    frames.first { $0.tNs > afterNs && !seen.contains($0.hash) }
}

/// The first frame for which `shows` holds, assuming that once the answer
/// is drawn it stays drawn; checked against the last frame first, so an
/// answer that never appeared is reported as missing, not as the last frame.
func firstShowing(_ frames: [Frame], _ shows: (CGImage) -> Bool) -> Frame? {
    let withImages = frames.filter { $0.image != nil }
    guard let last = withImages.last, shows(last.image!) else { return nil }
    var lo = 0
    var hi = withImages.count - 1
    while lo < hi {
        let mid = (lo + hi) / 2
        if shows(withImages[mid].image!) { hi = mid } else { lo = mid + 1 }
    }
    return withImages[lo]
}

func stats(_ values: [Double]) -> [String: Double] {
    guard !values.isEmpty else { return [:] }
    let s = values.sorted()
    func pct(_ f: Double) -> Double { s[max(0, Int((Double(s.count) * f).rounded(.up)) - 1)] }
    return ["n": Double(s.count), "median": pct(0.5), "p90": pct(0.9), "p95": pct(0.95), "p99": pct(0.99), "max": s.last!, "min": s.first!]
}

/// US key codes for the characters a query may hold.
let keyCodes: [Character: (CGKeyCode, Bool)] = {
    var m: [Character: (CGKeyCode, Bool)] = [:]
    let letters: [(Character, CGKeyCode)] = [("a", 0), ("s", 1), ("d", 2), ("f", 3), ("h", 4), ("g", 5), ("z", 6), ("x", 7), ("c", 8), ("v", 9), ("b", 11), ("q", 12), ("w", 13), ("e", 14), ("r", 15), ("y", 16), ("t", 17), ("o", 31), ("u", 32), ("i", 34), ("p", 35), ("l", 37), ("j", 38), ("k", 40), ("n", 45), ("m", 46)]
    for (c, k) in letters {
        m[c] = (k, false)
        m[Character(c.uppercased())] = (k, true)
    }
    let digits: [(Character, CGKeyCode)] = [("1", 18), ("2", 19), ("3", 20), ("4", 21), ("6", 22), ("5", 23), ("9", 25), ("7", 26), ("8", 28), ("0", 29)]
    for (c, k) in digits { m[c] = (k, false) }
    m[" "] = (49, false)
    m["-"] = (27, false)
    m["\""] = (39, true)
    m["'"] = (39, false)
    m["."] = (47, false)
    return m
}()

let kReturn: CGKeyCode = 36, kEscape: CGKeyCode = 53, kDelete: CGKeyCode = 51, kDown: CGKeyCode = 125

// MARK: recording

final class Recorder: NSObject, SCStreamOutput, @unchecked Sendable {
    let queue = DispatchQueue(label: "bench.frames")
    private var frames: [Frame] = []
    private var keepImages = false
    private var imageCount = 0
    let maxImages = 160
    private var seenWithImage = Set<UInt64>()

    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen,
              let attachments = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false) as? [[SCStreamFrameInfo: Any]],
              let a = attachments.first,
              let raw = a[.status] as? Int, SCFrameStatus(rawValue: raw) == .complete,
              let pb = CMSampleBufferGetImageBuffer(sb) else { return }
        let t = machToNs((a[.displayTime] as? UInt64) ?? mach_absolute_time())
        let h = hash(pb)
        var image: CGImage?
        if keepImages, imageCount < maxImages, !seenWithImage.contains(h) {
            VTCreateCGImageFromCVPixelBuffer(pb, options: nil, imageOut: &image)
            if image != nil { imageCount += 1; seenWithImage.insert(h) }
        }
        frames.append(Frame(tNs: t, hash: h, image: image))
    }

    func hash(_ pb: CVPixelBuffer) -> UInt64 {
        CVPixelBufferLockBaseAddress(pb, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(pb, .readOnly) }
        guard let base = CVPixelBufferGetBaseAddress(pb) else { return 0 }
        let w = CVPixelBufferGetWidth(pb), h = CVPixelBufferGetHeight(pb)
        let row = CVPixelBufferGetBytesPerRow(pb)
        let p = base.assumingMemoryBound(to: UInt8.self)
        var acc: UInt64 = 0xcbf29ce484222325
        var y = 0
        while y < h {
            var x = 0
            while x < w {
                let o = y * row + x * 4
                // Top bits only: a glyph or caret changes them, noise does not.
                acc = (acc ^ UInt64(p[o] >> 3 | (p[o + 1] >> 3) << 5)) &* 0x100000001b3
                acc = (acc ^ UInt64(p[o + 2] >> 3)) &* 0x100000001b3
                x += 2
            }
            y += 2
        }
        return acc
    }

    func snapshot() -> [Frame] { queue.sync { frames } }
    func lastHash() -> UInt64? { queue.sync { frames.last?.hash } }
    func setKeepImages(_ on: Bool) { queue.sync { keepImages = on; if on { imageCount = 0; seenWithImage = [] } } }
    func clear() { queue.sync { frames.removeAll(keepingCapacity: true) } }
    func latestImage() -> CGImage? { queue.sync { frames.last(where: { $0.image != nil })?.image } }
}

// MARK: the tool

@main
enum Interact {
    nonisolated(unsafe) static var tree: Set<pid_t> = []
    nonisolated(unsafe) static var appPid: pid_t = 0

    static func main() async {
        var args = Array(CommandLine.arguments.dropFirst())
        if args == ["--self-test"] { selfTest() }
        guard let mode = args.first else { fail("usage: interact preflight | typing|switcher|search|scroll --pid P ...") }
        args.removeFirst()
        if mode == "preflight" {
            // Which permissions the measuring Terminal holds; changes nothing.
            let ax = AXIsProcessTrusted(), sr = CGPreflightScreenCaptureAccess()
            emit(["schema": "krate.benchmark.interact.v1", "mode": "preflight", "accessibility": ax, "screen_recording": sr])
            exit(ax && sr ? 0 : 3)
        }
        var o: [String: String] = [:]
        while !args.isEmpty {
            let k = args.removeFirst()
            guard k.hasPrefix("--"), !args.isEmpty else { fail("bad option \(k)") }
            o[String(k.dropFirst(2))] = args.removeFirst()
        }
        guard let pidText = o["pid"], let root = pid_t(pidText) else { fail("--pid is required") }
        guard AXIsProcessTrusted() else {
            emit(["schema": "krate.benchmark.interact.v1", "mode": mode, "status": "refused",
                  "reason": "Accessibility permission is missing for the app running this (Terminal): System Settings > Privacy & Security > Accessibility"])
            exit(3)
        }
        guard CGPreflightScreenCaptureAccess() else {
            emit(["schema": "krate.benchmark.interact.v1", "mode": mode, "status": "refused",
                  "reason": "Screen Recording permission is missing for Terminal: System Settings > Privacy & Security > Screen Recording"])
            exit(3)
        }
        tree = Set(processTree(root).map(\.pid))
        guard let win = treeWindow(root) else { fail("no on-screen window in the tree of \(root)") }
        appPid = win.pid
        NSRunningApplication(processIdentifier: win.pid)?.activate()
        pause(0.8)

        let recorder = Recorder()
        let stream: SCStream
        do {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            guard let scw = content.windows.first(where: { Int($0.windowID) == win.id }) else { fail("ScreenCaptureKit does not list window \(win.id)") }
            let config = SCStreamConfiguration()
            config.width = Int(win.bounds.width)
            config.height = Int(win.bounds.height)
            config.minimumFrameInterval = CMTime(value: 1, timescale: 120)
            config.pixelFormat = kCVPixelFormatType_32BGRA
            config.showsCursor = false
            config.queueDepth = 8
            stream = SCStream(filter: SCContentFilter(desktopIndependentWindow: scw), configuration: config, delegate: nil)
            try stream.addStreamOutput(recorder, type: .screen, sampleHandlerQueue: recorder.queue)
            try await stream.startCapture()
        } catch {
            fail("could not record the window: \(error)")
        }
        pause(0.5)

        var result: [String: Any] = ["schema": "krate.benchmark.interact.v1", "mode": mode, "pid": Int(root),
                                     "window_bounds": boundsDict(win.bounds), "status": "accepted", "reason": "-"]
        switch mode {
        case "typing": typing(recorder, o, &result)
        case "switcher": switcher(recorder, o, &result)
        case "search": search(recorder, o, &result)
        case "scroll": scroll(recorder, win, o, &result)
        default: fail("unknown mode \(mode)")
        }
        try? await stream.stopCapture()
        result["ocr_level"] = ocrLevel
        emit(result)
        exit((result["status"] as? String) == "accepted" ? 0 : 1)
    }

    // MARK: input

    static func guardFocus() {
        guard let front = frontmostPid(), tree.contains(front) || front == appPid else {
            emit(["schema": "krate.benchmark.interact.v1", "status": "rejected", "reason": "focus_lost",
                  "frontmost_pid": Int(frontmostPid() ?? 0)])
            exit(4)
        }
    }

    @discardableResult
    static func key(_ code: CGKeyCode, _ flags: CGEventFlags = []) -> UInt64 {
        guardFocus()
        let src = CGEventSource(stateID: .hidSystemState)
        let down = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: true)!
        let up = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false)!
        down.flags = flags
        up.flags = flags
        let t = machNowNs()
        down.post(tap: .cghidEventTap)
        up.post(tap: .cghidEventTap)
        return t
    }

    /// Types `text` one key at a time; the post times of the first and last key.
    static func type(_ text: String, gapMs: Double = 40) -> (UInt64, UInt64) {
        var first: UInt64 = 0, last: UInt64 = 0
        for ch in text {
            guard let (code, shift) = keyCodes[ch] else { fail("no key code for \(ch)") }
            last = key(code, shift ? .maskShift : [])
            if first == 0 { first = last }
            pause(gapMs / 1000)
        }
        return (first, last)
    }

    /// Frames seen over `seconds` of no input, plus the last one before.
    static func learn(_ r: Recorder, seconds: Double) -> Set<UInt64> {
        var seen = Set<UInt64>()
        if let h = r.lastHash() { seen.insert(h) }
        let start = machNowNs()
        pause(seconds)
        for f in r.snapshot() where f.tNs >= start { seen.insert(f.hash) }
        return seen
    }

    static func waitNew(_ r: Recorder, after t: UInt64, seen: Set<UInt64>, timeout: Double) -> Frame? {
        let deadline = nowNs() + UInt64(timeout * 1e9)
        while nowNs() < deadline {
            if let f = firstNewFrame(r.snapshot(), after: t, seen: seen) { return f }
            pause(0.002)
        }
        return nil
    }

    static func ms(_ a: UInt64, _ b: UInt64) -> Double { (Double(a) - Double(b)) / 1e6 }

    static func showing(_ name: String) -> (CGImage) -> Bool {
        { contains(recognizedLines($0), name) }
    }

    // MARK: scenarios

    static func openViaSwitcher(_ name: String) {
        key(31, .maskCommand)
        pause(0.6)
        _ = type(name)
        pause(0.8)
        key(kReturn)
        pause(2.5)
    }

    static func typing(_ r: Recorder, _ o: [String: String], _ result: inout [String: Any]) {
        let n = Int(o["keys"] ?? "30") ?? 30
        if let note = o["note"] { openViaSwitcher(note) }
        key(kDown, .maskCommand)
        pause(1.0)
        var latencies: [Double] = []
        var missed = 0
        for i in 0..<n {
            let seen = learn(r, seconds: 1.4)
            let t0 = key(7)
            if let f = waitNew(r, after: t0, seen: seen, timeout: 1.0) {
                latencies.append(ms(f.tNs, t0))
            } else {
                missed += 1
            }
            pause(0.15 + Double(i % 3) * 0.05)
            r.clear()
        }
        result["keys"] = n
        result["latencies_ms"] = latencies
        result["missed"] = missed
        result["stats_ms"] = stats(latencies)
        if missed * 5 > n {
            result["status"] = "rejected"
            result["reason"] = "no_echo_for_\(missed)_of_\(n)_keys"
        }
    }

    static func switcher(_ r: Recorder, _ o: [String: String], _ result: inout [String: Any]) {
        guard let query = o["query"], let expect = o["expect"] else { fail("switcher needs --query and --expect") }
        r.setKeepImages(true)
        pause(0.3)
        if let img = r.latestImage(), showing(expect)(img) {
            result["status"] = "rejected"; result["reason"] = "expected_text_visible_before"; return
        }
        r.setKeepImages(false)
        let seen = learn(r, seconds: 1.2)
        let t0 = key(31, .maskCommand)
        guard let opened = waitNew(r, after: t0, seen: seen, timeout: 2.0) else {
            result["status"] = "rejected"; result["reason"] = "switcher_did_not_open"; return
        }
        result["switcher_open_ms"] = ms(opened.tNs, t0)
        pause(0.5)
        r.clear()
        r.setKeepImages(true)
        let (first, last) = type(query)
        pause(Double(o["seconds"] ?? "3") ?? 3)
        r.setKeepImages(false)
        let frames = r.snapshot().filter { $0.tNs >= first }
        guard let hit = firstShowing(frames, showing(expect)) else {
            result["status"] = "rejected"; result["reason"] = "expected_result_not_on_screen"
            key(kEscape); return
        }
        result["result_after_last_key_ms"] = ms(hit.tNs, last)
        result["result_after_first_key_ms"] = ms(hit.tNs, first)
        result["frames_recorded"] = frames.count
        if let file = o["phrases"] {
            let p = Phrases(file: file)
            r.clear()
            r.setKeepImages(true)
            let t1 = key(kReturn)
            pause(Double(o["open-seconds"] ?? "5") ?? 5)
            r.setKeepImages(false)
            if let shown = firstShowing(r.snapshot(), { p.matched(recognizedLines($0)).count >= 3 }) {
                result["open_note_ms"] = ms(shown.tNs, t1)
            } else {
                result["status"] = "rejected"; result["reason"] = "opened_note_not_on_screen"
            }
        } else {
            key(kEscape)
        }
    }

    static func search(_ r: Recorder, _ o: [String: String], _ result: inout [String: Any]) {
        guard let query = o["query"], let expect = o["expect"] else { fail("search needs --query and --expect") }
        key(3, [.maskCommand, .maskShift])
        pause(0.8)
        key(0, .maskCommand)
        key(kDelete)
        pause(1.5)
        r.setKeepImages(true)
        pause(0.3)
        if let img = r.latestImage(), showing(expect)(img) {
            result["status"] = "rejected"; result["reason"] = "expected_text_visible_before"; return
        }
        r.clear()
        r.setKeepImages(true)
        let (first, last) = type(query)
        pause(Double(o["seconds"] ?? "8") ?? 8)
        r.setKeepImages(false)
        let frames = r.snapshot().filter { $0.tNs >= first }
        guard let hit = firstShowing(frames, showing(expect)) else {
            result["status"] = "rejected"; result["reason"] = "expected_result_not_on_screen"; return
        }
        result["result_after_last_key_ms"] = ms(hit.tNs, last)
        result["result_after_first_key_ms"] = ms(hit.tNs, first)
        result["frames_recorded"] = frames.count
        key(kEscape)
    }

    static func scroll(_ r: Recorder, _ win: WindowRow, _ o: [String: String], _ result: inout [String: Any]) {
        let seconds = Double(o["seconds"] ?? "20") ?? 20
        let hz = Double(o["hz"] ?? "125") ?? 125
        let src = CGEventSource(stateID: .hidSystemState)
        let at = CGPoint(x: win.bounds.midX, y: win.bounds.minY + win.bounds.height * 0.5)
        guardFocus()
        CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: at, mouseButton: .left)?.post(tap: .cghidEventTap)
        pause(0.5)
        r.clear()
        var delta: Int32 = -3
        let interval = 1.0 / hz
        let total = Int(seconds * hz)
        let start = machNowNs()
        var posted = 0
        var flipped = false
        for i in 0..<total {
            if i % 25 == 0 { guardFocus() }
            if !flipped, i == Int(hz / 2), r.snapshot().isEmpty {
                // Nothing moved in half a second: the other wheel direction
                // is "down" on this Mac. Said in the result.
                delta = -delta
                flipped = true
            }
            guard let ev = CGEvent(scrollWheelEvent2Source: src, units: .pixel, wheelCount: 1, wheel1: delta, wheel2: 0, wheel3: 0) else { fail("no scroll event") }
            ev.location = at
            ev.post(tap: .cghidEventTap)
            posted += 1
            let target = start + UInt64(Double(i + 1) * interval * 1e9)
            while true {
                let now = machNowNs()
                if now >= target { break }
                let left = target - now
                if left > 1_000_000 { Thread.sleep(forTimeInterval: Double(left - 500_000) / 1e9) }
            }
        }
        let end = machNowNs()
        pause(0.3)
        let frames = r.snapshot().filter { $0.tNs >= start && $0.tNs <= end + 100_000_000 }
        var gaps: [Double] = []
        for i in 1..<max(1, frames.count) { gaps.append(ms(frames[i].tNs, frames[i - 1].tNs)) }
        let wall = Double(end - start) / 1e9
        result["posted_events"] = posted
        result["requested_hz"] = hz
        result["actual_hz"] = Double(posted) / wall
        result["duration_seconds"] = wall
        result["wheel_delta"] = Int(delta)
        result["direction_flipped"] = flipped
        result["changed_frames"] = frames.count
        result["frames_per_second"] = Double(frames.count) / wall
        result["frame_gap_ms"] = stats(gaps)
        if frames.count < 2 {
            result["status"] = "rejected"; result["reason"] = "no_frames_changed_while_scrolling"
        }
    }

    // MARK: self-test

    static func selfTest() -> Never {
        var failures: [String] = []
        // A caret blinking between two pictures, then the echo of a key.
        let blinkOn: UInt64 = 1, blinkOff: UInt64 = 2, echo: UInt64 = 3
        let frames = [Frame(tNs: 100, hash: blinkOn), Frame(tNs: 700, hash: blinkOff), Frame(tNs: 1300, hash: blinkOn),
                      Frame(tNs: 1500, hash: blinkOff), Frame(tNs: 1540, hash: echo)]
        if firstNewFrame(frames, after: 1400, seen: [blinkOn, blinkOff])?.tNs != 1540 {
            failures.append("a blink after the key is not the echo; the first unseen picture is")
        }
        if firstNewFrame(frames, after: 1600, seen: [blinkOn, blinkOff]) != nil {
            failures.append("nothing new after the last frame is none, not a stale frame")
        }
        // Result lists drawn as frames: empty, partial, then the answer.
        let images = [drawnText(["Quick switcher"]), drawnText(["Kite plan 09999", "Kite notes 00410"]),
                      drawnText(["Kite plan 10000", "Kite plan 09999"]), drawnText(["Kite plan 10000"])]
        let fs = images.enumerated().map { Frame(tNs: UInt64($0.offset * 10), hash: UInt64($0.offset), image: $0.element) }
        if firstShowing(fs, showing("Kite plan 10000"))?.tNs != 20 {
            failures.append("the first frame showing the answer is found (frame 3 of 4)")
        }
        if firstShowing(fs, showing("Tundra brief 00001")) != nil {
            failures.append("an answer never drawn is missing, not the last frame")
        }
        for q in ["\"after did back since just\"", "kite plan 10000", "Tundra brief 00001"] {
            for ch in q where keyCodes[ch] == nil { failures.append("no key code for \(ch) in \(q)") }
        }
        let s = stats([8, 8, 9, 8, 50, 8, 8, 9, 8, 8])
        if s["median"] != 8 || s["max"] != 50 || s["p90"] != 9 { failures.append("frame gap stats: \(s)") }
        if failures.isEmpty {
            print("interact self-test OK (OCR \(ocrLevel)) -- caret blink is not an echo, the first frame drawing the answer is found, missing answers are missing, every query character has a key")
            exit(0)
        }
        for f in failures { print("FAIL \(f)") }
        exit(1)
    }
}
