// Is the document on screen? (IC-822, portable tests 1744-1746.)
//
// A window is not content. This captures an app's window (screencapture
// -l, the system's own tool) and reads it with Apple's Vision text
// recognition, looking for the benchmark document's own lines --
// "Line <n>: the quick brown fox ..." (fixtures/generate.py). Content is
// on screen when at least two of those lines are read; the answer names
// the line numbers read, so a scroll leg can say whether the content
// moved.
//
//   content-probe --pid PID [--timeout SECONDS]   wait for content, then answer
//   content-probe --pid PID --once                 answer for what is there now
//   content-probe --self-test                      read a drawn line; no screen
//
// Needs Screen Recording permission for the terminal that runs it.

import AppKit
import Foundation
import Vision

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("ERROR: \(message)\n".utf8))
    exit(2)
}

func window(for pid: pid_t) -> Int? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let rows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
        return nil
    }
    // The app's largest normal window: an Electron app has helper windows.
    var best: (Int, Double)?
    for row in rows {
        guard let owner = row[kCGWindowOwnerPID as String] as? Int, owner == Int(pid) else { continue }
        guard (row[kCGWindowLayer as String] as? Int ?? -1) == 0 else { continue }
        let bounds = row[kCGWindowBounds as String] as? [String: Double] ?? [:]
        let area = (bounds["Width"] ?? 0) * (bounds["Height"] ?? 0)
        let number = row[kCGWindowNumber as String] as? Int ?? 0
        if best == nil || area > best!.1 { best = (number, area) }
    }
    return best?.0
}

/// Every window owned by the process or any of its children.
func pids(under root: pid_t) -> [pid_t] {
    let out = Pipe()
    let ps = Process()
    ps.executableURL = URL(fileURLWithPath: "/bin/ps")
    ps.arguments = ["-axo", "pid=,ppid="]
    ps.standardOutput = out
    try? ps.run()
    ps.waitUntilExit()
    let text = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    var children: [pid_t: [pid_t]] = [:]
    for line in text.split(separator: "\n") {
        let parts = line.split(separator: " ", omittingEmptySubsequences: true)
        if parts.count == 2, let pid = pid_t(parts[0]), let ppid = pid_t(parts[1]) {
            children[ppid, default: []].append(pid)
        }
    }
    var all = [root]
    var i = 0
    while i < all.count {
        all.append(contentsOf: children[all[i]] ?? [])
        i += 1
    }
    return all
}

func capture(_ windowId: Int) -> CGImage? {
    let file = FileManager.default.temporaryDirectory
        .appendingPathComponent("content-probe-\(getpid())-\(windowId).png")
    let shot = Process()
    shot.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    shot.arguments = ["-x", "-o", "-l\(windowId)", file.path]
    try? shot.run()
    shot.waitUntilExit()
    defer { try? FileManager.default.removeItem(at: file) }
    guard shot.terminationStatus == 0,
          let image = NSImage(contentsOf: file),
          let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil)
    else { return nil }
    return cg
}

/// The benchmark document's line numbers read in an image.
func documentLines(in image: CGImage) -> [Int] {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    let handler = VNImageRequestHandler(cgImage: image, options: [:])
    try? handler.perform([request])
    let pattern = try! NSRegularExpression(pattern: #"Line\s+(\d+)\s*[:;]?\s*the\s+quick"#, options: [.caseInsensitive])
    var found: [Int] = []
    for observation in request.results ?? [] {
        guard let text = observation.topCandidates(1).first?.string else { continue }
        let range = NSRange(text.startIndex..., in: text)
        for match in pattern.matches(in: text, range: range) {
            if let r = Range(match.range(at: 1), in: text), let n = Int(text[r]) {
                found.append(n)
            }
        }
    }
    return found.sorted()
}

func answer(_ value: [String: Any]) -> Never {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
    exit((value["content_on_screen"] as? Bool) == true ? 0 : 1)
}

func selfTest() -> Never {
    let width = 900, height = 160
    let space = CGColorSpaceCreateDeviceRGB()
    guard let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                              space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        fail("no drawing context")
    }
    ctx.setFillColor(NSColor.white.cgColor)
    ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
    let graphics = NSGraphicsContext(cgContext: ctx, flipped: false)
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = graphics
    let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 22), .foregroundColor: NSColor.black]
    ("Line 41: the quick brown fox jumps over the lazy dog" as NSString).draw(at: NSPoint(x: 20, y: 95), withAttributes: attrs)
    ("Line 42: the quick brown fox jumps over the lazy dog" as NSString).draw(at: NSPoint(x: 20, y: 40), withAttributes: attrs)
    NSGraphicsContext.restoreGraphicsState()
    guard let image = ctx.makeImage() else { fail("no image") }
    let lines = documentLines(in: image)
    let blank = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                          space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    blank.setFillColor(NSColor.white.cgColor)
    blank.fill(CGRect(x: 0, y: 0, width: width, height: height))
    let none = documentLines(in: blank.makeImage()!)
    guard lines == [41, 42], none.isEmpty else {
        fail("self-test read \(lines) from two drawn lines and \(none) from a blank image")
    }
    print("content-probe self-test OK -- two drawn document lines read as 41 and 42, a blank window as none")
    exit(0)
}

var args = Array(CommandLine.arguments.dropFirst())
if args == ["--self-test"] { selfTest() }
var pid: pid_t?
var timeout = 30.0
var once = false
while !args.isEmpty {
    let flag = args.removeFirst()
    switch flag {
    case "--pid": pid = pid_t(args.isEmpty ? "" : args.removeFirst())
    case "--timeout": timeout = Double(args.isEmpty ? "" : args.removeFirst()) ?? 0
    case "--once": once = true
    default: fail("unknown option \(flag)")
    }
}
guard let root = pid, timeout > 0 else { fail("usage: content-probe --pid PID [--timeout S] [--once]") }

let started = DispatchTime.now().uptimeNanoseconds
let deadline = started + UInt64(timeout * 1_000_000_000)
var last: [String: Any] = ["schema": "krate.benchmark.content.v1", "pid": Int(root), "content_on_screen": false]
repeat {
    for p in pids(under: root) {
        guard let id = window(for: p) else { continue }
        guard let image = capture(id) else {
            last["problem"] = "the window could not be captured; give the terminal Screen Recording permission"
            continue
        }
        let lines = documentLines(in: image)
        last["window_id"] = id
        last["lines_read"] = lines.count
        last["first_line"] = lines.first ?? 0
        last["last_line"] = lines.last ?? 0
        if lines.count >= 2 {
            last["content_on_screen"] = true
            last["content_ms"] = Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
            last.removeValue(forKey: "problem")
            answer(last)
        }
    }
    if once { break }
    Thread.sleep(forTimeInterval: 0.25)
} while DispatchTime.now().uptimeNanoseconds < deadline
answer(last)
