// start-probe: launch an app and time it from outside, the same way for
// both apps (IC-999).
//
//   start-probe [--timeout S] [--phrases FILE] [--min-phrases N]
//               [--child-log FILE] [--terminate] -- COMMAND [ARG ...]
//       Launch COMMAND (stdin /dev/null, stdout and stderr to the log).
//       window_ms: launch to the first on-screen layer-0 window of the
//       process tree. content_ms: launch to the end of the first window
//       capture in which at least N of the note's phrases were read;
//       content_lower_ms is the start of the capture before it, so the note
//       appeared between the two. Without --terminate the app is left
//       running and its pid printed, for the legs that keep measuring.
//
//   start-probe --attach PID --phrases FILE [--timeout S | --once]
//       Is the note on screen in PID's tree now (or within S seconds)?
//
//   start-probe --self-test
//       Phrase matching on drawn text; no screen, no app.
//
// Exit 0 when the window (and the note, if --phrases) was seen, else 1.

import AppKit
import Foundation

@main
enum StartProbe {
    static func main() {
        var args = Array(CommandLine.arguments.dropFirst())
        if args == ["--self-test"] { selfTest() }
        var timeout = 60.0
        var phrasesFile: String?
        var minPhrases = 3
        var childLog = "/dev/null"
        var terminate = false
        var attach: pid_t?
        var once = false
        var command: [String] = []
        while !args.isEmpty {
            let a = args.removeFirst()
            func value() -> String {
                guard !args.isEmpty else { fail("\(a) needs a value") }
                return args.removeFirst()
            }
            switch a {
            case "--timeout": timeout = Double(value()) ?? 0
            case "--phrases": phrasesFile = value()
            case "--min-phrases": minPhrases = Int(value()) ?? 3
            case "--child-log": childLog = value()
            case "--terminate": terminate = true
            case "--attach": attach = pid_t(value())
            case "--once": once = true
            case "--": command = args; args = []
            default: fail("unknown option \(a)")
            }
        }
        guard timeout > 0 else { fail("--timeout needs a positive number") }
        let phrases = phrasesFile.map { Phrases(file: $0) }
        if let pid = attach {
            guard let p = phrases else { fail("--attach needs --phrases") }
            attachMode(pid, p, minPhrases, once ? 0 : timeout)
        }
        guard !command.isEmpty else { fail("usage: start-probe [options] -- COMMAND [ARG ...]") }
        launchMode(command, phrases, minPhrases, timeout, childLog, terminate)
    }

    static func launchMode(_ command: [String], _ phrases: Phrases?, _ minPhrases: Int,
                           _ timeout: Double, _ childLog: String, _ terminate: Bool) -> Never {
        FileManager.default.createFile(atPath: childLog, contents: nil)
        guard let log = FileHandle(forWritingAtPath: childLog) else { fail("cannot write \(childLog)") }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: command[0])
        process.arguments = Array(command.dropFirst())
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = log
        process.standardError = log
        var result: [String: Any] = [
            "schema": "krate.benchmark.start.v2",
            "command": command,
            "poll_window_ms": 15,
            "content_on_screen": false,
        ]
        let started = nowNs()
        do { try process.run() } catch { fail("could not launch \(command[0]): \(error)") }
        let pid = process.processIdentifier
        result["pid"] = Int(pid)
        result["started_utc"] = ISO8601DateFormatter().string(from: Date())
        let deadline = started + UInt64(timeout * 1e9)
        func ms(_ t: UInt64) -> Double { Double(t - started) / 1e6 }

        var window: WindowRow?
        var captures = 0
        var previousCaptureStart: UInt64?
        while nowNs() < deadline {
            if !process.isRunning {
                result["exit_status"] = Int(process.terminationStatus)
                break
            }
            if window == nil, let w = treeWindow(pid) {
                window = w
                result["window_ms"] = ms(nowNs())
                result["window_id"] = w.id
                result["window_pid"] = Int(w.pid)
                result["window_owner"] = w.owner
                result["window_bounds"] = boundsDict(w.bounds)
                if phrases == nil { break }
            }
            guard let p = phrases, let w = window else {
                Thread.sleep(forTimeInterval: 0.015)
                continue
            }
            // The window may be replaced (a splash, then the main window).
            let current = treeWindow(pid) ?? w
            let captureStart = nowNs()
            guard let image = captureWindow(current.id) else {
                result["problem"] = "the window could not be captured; give Terminal Screen Recording permission"
                Thread.sleep(forTimeInterval: 0.1)
                continue
            }
            let captureEnd = nowNs()
            captures += 1
            let lines = p.matched(recognizedLines(image))
            result["lines_read"] = lines.count
            result["first_line"] = lines.first ?? 0
            result["last_line"] = lines.last ?? 0
            if lines.count >= minPhrases {
                result["content_on_screen"] = true
                result["content_ms"] = ms(captureEnd)
                result["content_lower_ms"] = ms(previousCaptureStart ?? captureStart)
                result["content_window_bounds"] = boundsDict(current.bounds)
                result["lines"] = Array(lines.prefix(50))
                result.removeValue(forKey: "problem")
                break
            }
            previousCaptureStart = captureStart
        }
        result["captures"] = captures
        result["min_phrases"] = minPhrases
        result["ocr_level"] = ocrLevel
        let ok = window != nil && (phrases == nil || (result["content_on_screen"] as? Bool) == true)
        result["timed_out"] = !ok
        if terminate || !ok {
            // Ask the app to quit (SIGTERM to its main process, as a
            // normal quit does), give its tree 6 s to go, then stop what is left.
            let members = processTree(pid).map(\.pid)
            kill(pid, SIGTERM)
            let until = nowNs() + 6_000_000_000
            while nowNs() < until && members.contains(where: { kill($0, 0) == 0 }) {
                Thread.sleep(forTimeInterval: 0.1)
            }
            let left = members.filter { kill($0, 0) == 0 }
            for p in left { kill(p, SIGKILL) }
            result["killed_after_quit"] = left.count
            result["terminated"] = true
        }
        emit(result)
        exit(ok ? 0 : 1)
    }

    static func attachMode(_ root: pid_t, _ p: Phrases, _ minPhrases: Int, _ timeout: Double) -> Never {
        let started = nowNs()
        var result: [String: Any] = ["schema": "krate.benchmark.onscreen.v1", "pid": Int(root), "content_on_screen": false]
        repeat {
            if let w = treeWindow(root) {
                result["window_bounds"] = boundsDict(w.bounds)
                if let image = captureWindow(w.id) {
                    let lines = p.matched(recognizedLines(image))
                    result["lines_read"] = lines.count
                    result["first_line"] = lines.first ?? 0
                    result["last_line"] = lines.last ?? 0
                    result["lines"] = Array(lines.prefix(50))
                    if lines.count >= minPhrases {
                        result["content_on_screen"] = true
                        result["content_ms"] = Double(nowNs() - started) / 1e6
                        result.removeValue(forKey: "problem")
                        result["ocr_level"] = ocrLevel
                        emit(result)
                        exit(0)
                    }
                } else {
                    result["problem"] = "the window could not be captured; give Terminal Screen Recording permission"
                }
            } else {
                result["problem"] = "no on-screen window in the process tree"
            }
            if timeout == 0 { break }
            Thread.sleep(forTimeInterval: 0.25)
        } while Double(nowNs() - started) / 1e9 < timeout
        result["ocr_level"] = ocrLevel
        emit(result)
        exit(1)
    }

    static func selfTest() -> Never {
        let p = Phrases([
            "after did back since just": 3,
            "client each review health world": 4,
            "where notes do here idea": 11,
            "out take or schedule too": 10,
        ], size: 5)
        // A note drawn as an editor would: markup hidden, a link shown by
        // its text, a sentence wrapped over two lines.
        let note = drawnText([
            "Willow findings 00001",
            "After did back since just make sprint been very through",
            "could last only their Juniper brief 00020 very then did.",
            "Client each review health world work since idea with answer.",
            "Out take or schedule too health client of Alder notes 00024.",
        ])
        let read = p.matched(recognizedLines(note))
        // A sidebar showing only file names, and another note's lines.
        let other = drawnText(["Willow findings 00001", "Juniper brief 00020", "Client each review the world work"])
        let none = p.matched(recognizedLines(other))
        let blank = drawnText([])
        guard read == [3, 4, 10], none.isEmpty, p.matched(recognizedLines(blank)).isEmpty else {
            fail("self-test read \(read) from the note, \(none) from names and another note")
        }
        guard contains(recognizedLines(other), "Juniper brief 00020"),
              !contains(recognizedLines(other), "Kite plan 10000") else {
            fail("self-test could not tell a note name on screen from one that is not")
        }
        print("start-probe self-test OK (OCR \(ocrLevel)) -- three of the note's own phrases read (lines 3, 4, 10, one across a wrapped line); file names and another note's words read as none")
        exit(0)
    }
}
