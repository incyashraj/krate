// macOS-only local test driver, scoped to the supplied Hexview process.
import AppKit
import CoreGraphics

let pid = pid_t(CommandLine.arguments[1])!
let command = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "list"
if command != "list" && !CGPreflightPostEventAccess() {
    fputs("OS input automation permission is unavailable; no events sent.\n", stderr)
    exit(2)
}
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
let own = windows.filter { ($0[kCGWindowOwnerPID as String] as? Int) == Int(pid) }
if command == "list" {
    for w in own { print(w) }
} else if command == "key" {
    let code = CGKeyCode(CommandLine.arguments[3])!
    let down = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)!
    let up = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)!
    if CommandLine.arguments.contains("cmd") { down.flags = .maskCommand; up.flags = .maskCommand }
    NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateIgnoringOtherApps])
    down.postToPid(pid); up.postToPid(pid)
} else if command == "type" {
    let chars = Array(CommandLine.arguments[3].utf16)
    let down = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true)!
    down.keyboardSetUnicodeString(stringLength: chars.count, unicodeString: chars)
    let up = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false)!
    NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateIgnoringOtherApps])
    down.postToPid(pid); up.postToPid(pid)
} else if command == "click" {
    guard let w = own.first(where: { ($0[kCGWindowLayer as String] as? Int) == 0 }),
          let b = w[kCGWindowBounds as String] as? [String: Double] else { fatalError("No app window") }
    let point = CGPoint(x: b["X"]! + Double(CommandLine.arguments[3])!,
                        y: b["Y"]! + Double(CommandLine.arguments[4])!)
    NSRunningApplication(processIdentifier: pid)?.activate(options: [.activateIgnoringOtherApps])
    CGEvent(mouseEventSource: nil, mouseType: .leftMouseDown, mouseCursorPosition: point, mouseButton: .left)!.postToPid(pid)
    CGEvent(mouseEventSource: nil, mouseType: .leftMouseUp, mouseCursorPosition: point, mouseButton: .left)!.postToPid(pid)
}
