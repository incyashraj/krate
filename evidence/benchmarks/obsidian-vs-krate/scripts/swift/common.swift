// Shared code for the Obsidian-versus-Krate instruments (IC-999).
//
// Process trees from the kernel's own process table, the app's window
// from the window server, OCR of a window with Apple's Vision, and the
// phrase matching that decides whether the target note is on screen.
// Each tool is compiled as `swiftc -parse-as-library common.swift
// <tool>.swift`, so this file has no top-level statements.

import AppKit
import Darwin
import Foundation
import Vision

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("ERROR: \(message)\n".utf8))
    exit(2)
}

func emit(_ value: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

// MARK: clocks

let timebase: mach_timebase_info_data_t = {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    return info
}()

func machToNs(_ ticks: UInt64) -> UInt64 {
    ticks &* UInt64(timebase.numer) / UInt64(timebase.denom)
}

func nowNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

/// DispatchTime.uptimeNanoseconds and mach_absolute_time count the same
/// clock; this converts a mach timestamp (ScreenCaptureKit's display
/// time, CGEvent times) to it.
func machNowNs() -> UInt64 { machToNs(mach_absolute_time()) }

// MARK: processes

struct ProcInfo {
    let pid: pid_t
    let ppid: pid_t
    let name: String
    let path: String
}

func allProcesses() -> [ProcInfo] {
    let capacity = 8192
    var pids = [pid_t](repeating: 0, count: capacity)
    let n = pids.withUnsafeMutableBytes { proc_listallpids($0.baseAddress, Int32($0.count)) }
    guard n > 0 else { return [] }
    var out: [ProcInfo] = []
    for pid in pids.prefix(Int(n)) where pid > 0 {
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.stride)
        guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { continue }
        let name = withUnsafeBytes(of: info.pbi_comm) { raw -> String in
            let bytes = raw.prefix { $0 != 0 }
            return String(decoding: bytes, as: UTF8.self)
        }
        var pathBuf = [CChar](repeating: 0, count: 4096)
        let len = proc_pidpath(pid, &pathBuf, UInt32(pathBuf.count))
        let path = len > 0 ? String(cString: pathBuf) : ""
        out.append(ProcInfo(pid: pid, ppid: pid_t(info.pbi_ppid), name: name, path: path))
    }
    return out
}

/// The root and every descendant by parent pid, root first.
func processTree(_ root: pid_t, table: [ProcInfo]? = nil) -> [ProcInfo] {
    let rows = table ?? allProcesses()
    var children: [pid_t: [ProcInfo]] = [:]
    var byPid: [pid_t: ProcInfo] = [:]
    for r in rows {
        children[r.ppid, default: []].append(r)
        byPid[r.pid] = r
    }
    guard let first = byPid[root] else { return [] }
    var out = [first]
    var i = 0
    while i < out.count {
        for c in children[out[i].pid] ?? [] where !out.contains(where: { $0.pid == c.pid }) {
            out.append(c)
        }
        i += 1
    }
    return out
}

// MARK: windows

struct WindowRow {
    let id: Int
    let pid: pid_t
    let owner: String
    let bounds: CGRect
}

/// On-screen layer-0 windows of any process in `pids`, largest first.
func windows(of pids: Set<pid_t>) -> [WindowRow] {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let rows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else { return [] }
    var out: [WindowRow] = []
    for row in rows {
        guard let owner = row[kCGWindowOwnerPID as String] as? Int, pids.contains(pid_t(owner)) else { continue }
        guard (row[kCGWindowLayer as String] as? Int ?? -1) == 0 else { continue }
        guard (row[kCGWindowAlpha as String] as? Double ?? 0) > 0 else { continue }
        let b = row[kCGWindowBounds as String] as? [String: Double] ?? [:]
        let rect = CGRect(x: b["X"] ?? 0, y: b["Y"] ?? 0, width: b["Width"] ?? 0, height: b["Height"] ?? 0)
        guard rect.width >= 100, rect.height >= 100 else { continue }
        out.append(WindowRow(id: row[kCGWindowNumber as String] as? Int ?? 0, pid: pid_t(owner),
                             owner: row[kCGWindowOwnerName as String] as? String ?? "",
                             bounds: rect))
    }
    return out.sorted { $0.bounds.width * $0.bounds.height > $1.bounds.width * $1.bounds.height }
}

func treeWindow(_ root: pid_t) -> WindowRow? {
    windows(of: Set(processTree(root).map(\.pid))).first
}

func boundsDict(_ r: CGRect) -> [String: Double] {
    ["x": Double(r.origin.x), "y": Double(r.origin.y), "width": Double(r.width), "height": Double(r.height)]
}

/// The window as an image, through the system's own `screencapture`.
/// Needs Screen Recording for the terminal; nil when it is refused.
func captureWindow(_ id: Int) -> CGImage? {
    let file = FileManager.default.temporaryDirectory
        .appendingPathComponent("bench-capture-\(getpid())-\(id)-\(nowNs()).png")
    let shot = Process()
    shot.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    shot.arguments = ["-x", "-o", "-l\(id)", file.path]
    shot.standardOutput = FileHandle.nullDevice
    shot.standardError = FileHandle.nullDevice
    do { try shot.run() } catch { return nil }
    shot.waitUntilExit()
    defer { try? FileManager.default.removeItem(at: file) }
    guard shot.terminationStatus == 0, let image = NSImage(contentsOf: file),
          let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return nil }
    return cg
}

// MARK: text

/// Lower case, every character other than a-z, 0-9 and '-' a space, then
/// words. note-phrases.py applies the same rule to the note's source.
func words(_ text: String) -> [String] {
    var out: [String] = []
    var cur = ""
    for ch in text.lowercased() {
        if ch.isASCII, ch.isLetter || ch.isNumber || ch == "-" {
            cur.append(ch)
        } else if !cur.isEmpty {
            out.append(cur)
            cur = ""
        }
    }
    if !cur.isEmpty { out.append(cur) }
    return out
}

/// Which recognition level the last OCR used. `accurate` needs the neural
/// engine service; where a sandbox refuses it (seen 2026-11-22 inside an
/// agent's command sandbox: e5rt error 13) the probe falls back to `fast`
/// and says so in its output rather than reading nothing.
nonisolated(unsafe) var ocrLevel = "accurate"
nonisolated(unsafe) var ocrProblem: String?

func recognizedLines(_ image: CGImage) -> [String] {
    var results: [VNRecognizedTextObservation] = []
    for level in [VNRequestTextRecognitionLevel.accurate, .fast] {
        if level == .accurate && ocrLevel == "fast" { continue }
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = level
        request.usesLanguageCorrection = false
        let handler = VNImageRequestHandler(cgImage: image, options: [:])
        do {
            try handler.perform([request])
            results = request.results ?? []
            break
        } catch {
            ocrProblem = "\(error)"
            ocrLevel = "fast"
        }
    }
    let obs = results.sorted {
        // Top to bottom (Vision's y grows upward), then left to right.
        abs($0.boundingBox.midY - $1.boundingBox.midY) > 0.005
            ? $0.boundingBox.midY > $1.boundingBox.midY
            : $0.boundingBox.minX < $1.boundingBox.minX
    }
    return obs.compactMap { $0.topCandidates(1).first?.string }
}

/// Every run of `n` consecutive words read, within one recognized line and
/// across neighbouring lines (a sentence that wraps).
func ngrams(_ lines: [String], n: Int) -> Set<String> {
    var grams = Set<String>()
    let perLine = lines.map(words)
    for w in perLine where w.count >= n {
        for i in 0...(w.count - n) { grams.insert(w[i..<(i + n)].joined(separator: " ")) }
    }
    let all = perLine.flatMap { $0 }
    if all.count >= n {
        for i in 0...(all.count - n) { grams.insert(all[i..<(i + n)].joined(separator: " ")) }
    }
    return grams
}

/// Phrases of the target note: phrase (5 normalised words) -> source line.
struct Phrases {
    let byPhrase: [String: Int]
    let size: Int

    init(file: String) {
        guard let text = try? String(contentsOfFile: file, encoding: .utf8) else { fail("cannot read phrases \(file)") }
        var map: [String: Int] = [:]
        var size = 5
        for line in text.split(separator: "\n") where !line.hasPrefix("#") {
            let parts = line.split(separator: "\t", maxSplits: 1)
            guard parts.count == 2, let n = Int(parts[0]) else { continue }
            let p = words(String(parts[1])).joined(separator: " ")
            size = words(String(parts[1])).count
            map[p] = n
        }
        guard !map.isEmpty else { fail("no phrases in \(file)") }
        self.byPhrase = map
        self.size = size
    }

    init(_ map: [String: Int], size: Int) {
        self.byPhrase = map
        self.size = size
    }

    /// Source line numbers whose phrase was read, sorted.
    func matched(_ lines: [String]) -> [Int] {
        let grams = ngrams(lines, n: size)
        return Array(Set(grams.compactMap { byPhrase[$0] })).sorted()
    }
}

/// Whether `expected` (a note name) is read as consecutive words.
func contains(_ lines: [String], _ expected: String) -> Bool {
    let want = words(expected)
    guard !want.isEmpty else { return false }
    return ngrams(lines, n: want.count).contains(want.joined(separator: " "))
}

/// A drawn image of text lines, for the self-tests (no screen needed).
func drawnText(_ lines: [String], width: Int = 1100, lineHeight: Int = 34) -> CGImage {
    let height = lineHeight * (lines.count + 1)
    let space = CGColorSpaceCreateDeviceRGB()
    guard let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                              space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else {
        fail("no drawing context")
    }
    ctx.setFillColor(NSColor.white.cgColor)
    ctx.fill(CGRect(x: 0, y: 0, width: width, height: height))
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
    let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 20), .foregroundColor: NSColor.black]
    for (i, line) in lines.enumerated() {
        (line as NSString).draw(at: NSPoint(x: 16, y: height - lineHeight * (i + 1)), withAttributes: attrs)
    }
    NSGraphicsContext.restoreGraphicsState()
    return ctx.makeImage()!
}

/// Frontmost application's pid, for refusing to post input to the wrong app.
func frontmostPid() -> pid_t? { NSWorkspace.shared.frontmostApplication?.processIdentifier }

/// Sleep that may be called from an async context without the compiler
/// asking for Task.sleep (the measuring threads here are plain threads).
func pause(_ seconds: Double) {
    guard seconds > 0 else { return }
    usleep(UInt32(min(seconds, 4000) * 1_000_000))
}
