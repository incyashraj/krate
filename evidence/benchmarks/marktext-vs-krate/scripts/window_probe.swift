import AppKit
import Foundation

struct Options {
    var timeoutSeconds: Double = 30
    var terminate = false
    var command: [String] = []
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("ERROR: \(message)\n".utf8))
    exit(2)
}

func parseOptions() -> Options {
    var result = Options()
    let args = Array(CommandLine.arguments.dropFirst())
    guard let separator = args.firstIndex(of: "--") else {
        fail("usage: window-probe [--timeout SECONDS] [--terminate] -- COMMAND [ARG ...]")
    }
    var index = 0
    while index < separator {
        switch args[index] {
        case "--timeout":
            index += 1
            guard index < separator, let value = Double(args[index]), value > 0 else {
                fail("--timeout needs a positive number")
            }
            result.timeoutSeconds = value
        case "--terminate":
            result.terminate = true
        default:
            fail("unknown option: \(args[index])")
        }
        index += 1
    }
    result.command = Array(args.dropFirst(separator + 1))
    guard !result.command.isEmpty else { fail("missing command after --") }
    return result
}

func visibleWindow(for pid: pid_t) -> (Int, String)? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let rows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
        return nil
    }
    for row in rows {
        guard let owner = row[kCGWindowOwnerPID as String] as? Int, owner == Int(pid) else { continue }
        let layer = row[kCGWindowLayer as String] as? Int ?? -1
        let alpha = row[kCGWindowAlpha as String] as? Double ?? 0
        guard layer == 0, alpha > 0 else { continue }
        let number = row[kCGWindowNumber as String] as? Int ?? 0
        let name = row[kCGWindowOwnerName as String] as? String ?? "unknown"
        return (number, name)
    }
    return nil
}

let options = parseOptions()
let process = Process()
process.executableURL = URL(fileURLWithPath: options.command[0])
process.arguments = Array(options.command.dropFirst())
process.standardOutput = FileHandle.standardOutput
process.standardError = FileHandle.standardError

let started = DispatchTime.now().uptimeNanoseconds
do {
    try process.run()
} catch {
    fail("could not launch \(options.command[0]): \(error)")
}

let deadline = started + UInt64(options.timeoutSeconds * 1_000_000_000)
var result: [String: Any] = [
    "schema": "krate.benchmark.window.v1",
    "pid": Int(process.processIdentifier),
    "command": options.command,
]

while DispatchTime.now().uptimeNanoseconds < deadline {
    if let window = visibleWindow(for: process.processIdentifier) {
        let elapsed = Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000
        result["window_ms"] = elapsed
        result["window_id"] = window.0
        result["window_owner"] = window.1
        result["timed_out"] = false
        break
    }
    if !process.isRunning {
        result["exit_status"] = Int(process.terminationStatus)
        break
    }
    Thread.sleep(forTimeInterval: 0.015)
}

if result["window_ms"] == nil {
    result["timed_out"] = true
}

if options.terminate, process.isRunning {
    process.terminate()
    Thread.sleep(forTimeInterval: 0.25)
    if process.isRunning {
        kill(process.processIdentifier, SIGKILL)
    }
}

let encoded = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
FileHandle.standardOutput.write(encoded)
FileHandle.standardOutput.write(Data("\n".utf8))

if result["timed_out"] as? Bool == true {
    exit(1)
}
