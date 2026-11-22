// tree-sample: what a process tree uses, from the kernel, sampled over time
// (IC-999). No sudo: proc_pid_rusage and proc_pidinfo answer for any
// process of the same user.
//
//   tree-sample --pid ROOT [--also-exe PATH ...] --interval S --count N
//       One JSON line per sample: every process in ROOT's tree (and any
//       other process whose executable path starts with an --also-exe
//       prefix, so a detached helper of the same app is counted), each
//       with phys_footprint, resident bytes, threads, open descriptors,
//       CPU time, idle and interrupt wakeups and the kernel's billed
//       energy counter, plus the sums. The first line is sample 0 at the
//       moment the tool starts; CPU % and wakeups per second come from the
//       differences between samples (analyze.py).
//
//   tree-sample --self-test
//       Samples itself and a child it starts; no app, no screen.

import Darwin
import Foundation

@main
enum TreeSample {
    static func usage(_ pid: pid_t) -> [String: Any]? {
        var info = rusage_info_v4()
        let rc = withUnsafeMutablePointer(to: &info) { ptr in
            ptr.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V4, $0) }
        }
        guard rc == 0 else { return nil }
        var task = proc_taskinfo()
        let tsize = Int32(MemoryLayout<proc_taskinfo>.stride)
        let threads = proc_pidinfo(pid, PROC_PIDTASKINFO, 0, &task, tsize) == tsize ? Int(task.pti_threadnum) : -1
        let fdBytes = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, nil, 0)
        var fds = -1
        if fdBytes > 0 {
            let cap = Int(fdBytes) / MemoryLayout<proc_fdinfo>.stride + 16
            var buf = [proc_fdinfo](repeating: proc_fdinfo(), count: cap)
            let got = buf.withUnsafeMutableBytes { proc_pidinfo(pid, PROC_PIDLISTFDS, 0, $0.baseAddress, Int32($0.count)) }
            if got >= 0 { fds = Int(got) / MemoryLayout<proc_fdinfo>.stride }
        }
        return [
            "phys_footprint": info.ri_phys_footprint,
            "resident": info.ri_resident_size,
            "threads": threads,
            "open_fds": fds,
            // ri_user_time / ri_system_time are mach absolute time units.
            "cpu_ns": machToNs(info.ri_user_time) + machToNs(info.ri_system_time),
            "idle_wakeups": info.ri_pkg_idle_wkups,
            "interrupt_wakeups": info.ri_interrupt_wkups,
            "billed_energy": info.ri_billed_energy,
            "instructions": info.ri_instructions,
        ]
    }

    static func sample(root: pid_t, alsoExe: [String], index: Int, started: UInt64) -> [String: Any] {
        let table = allProcesses()
        var members = processTree(root, table: table)
        let inTree = Set(members.map(\.pid))
        for p in table where !inTree.contains(p.pid) && alsoExe.contains(where: { !$0.isEmpty && p.path.hasPrefix($0) }) {
            members.append(p)
        }
        var procs: [[String: Any]] = []
        var sums: [String: UInt64] = [:]
        var threads = 0
        var fds = 0
        for m in members {
            guard var u = usage(m.pid) else { continue }
            u["pid"] = Int(m.pid)
            u["ppid"] = Int(m.ppid)
            u["name"] = m.name
            u["path"] = m.path
            u["in_tree"] = inTree.contains(m.pid)
            for k in ["phys_footprint", "resident", "cpu_ns", "idle_wakeups", "interrupt_wakeups", "billed_energy"] {
                sums[k, default: 0] &+= (u[k] as? UInt64) ?? 0
            }
            threads += max(0, u["threads"] as? Int ?? 0)
            fds += max(0, u["open_fds"] as? Int ?? 0)
            procs.append(u)
        }
        var out: [String: Any] = [
            "schema": "krate.benchmark.tree-sample.v1",
            "sample": index,
            "t_s": Double(nowNs() - started) / 1e9,
            "utc": ISO8601DateFormatter().string(from: Date()),
            "epoch": Date().timeIntervalSince1970,
            "root_pid": Int(root),
            "root_alive": kill(root, 0) == 0,
            "process_count": procs.count,
            "threads": threads,
            "open_fds": fds,
            "processes": procs,
        ]
        for (k, v) in sums { out[k] = v }
        return out
    }

    static func main() {
        var args = Array(CommandLine.arguments.dropFirst())
        if args == ["--self-test"] { selfTest() }
        var root: pid_t?
        var also: [String] = []
        var interval = 1.0
        var count = 1
        while !args.isEmpty {
            let a = args.removeFirst()
            guard !args.isEmpty else { fail("\(a) needs a value") }
            let v = args.removeFirst()
            switch a {
            case "--pid": root = pid_t(v)
            case "--also-exe": also.append(v)
            case "--interval": interval = Double(v) ?? 0
            case "--count": count = Int(v) ?? 0
            default: fail("unknown option \(a)")
            }
        }
        guard let pid = root, interval > 0, count > 0 else {
            fail("usage: tree-sample --pid ROOT [--also-exe PATH] --interval S --count N")
        }
        let started = nowNs()
        for i in 0..<count {
            let s = sample(root: pid, alsoExe: also, index: i, started: started)
            emit(s)
            if (s["root_alive"] as? Bool) != true { exit(1) }
            if i + 1 < count {
                let target = started + UInt64(Double(i + 1) * interval * 1e9)
                let now = nowNs()
                if target > now { Thread.sleep(forTimeInterval: Double(target - now) / 1e9) }
            }
        }
        exit(0)
    }

    static func selfTest() -> Never {
        let child = Process()
        child.executableURL = URL(fileURLWithPath: "/bin/sleep")
        child.arguments = ["5"]
        try! child.run()
        defer { child.terminate() }
        Thread.sleep(forTimeInterval: 0.2)
        let started = nowNs()
        let a = sample(root: getpid(), alsoExe: [], index: 0, started: started)
        // Burn a little CPU so the second sample must show more CPU time.
        var x = 0.0
        for i in 0..<20_000_000 { x += Double(i % 7) }
        let b = sample(root: getpid(), alsoExe: [], index: 1, started: started)
        let procs = a["processes"] as? [[String: Any]] ?? []
        let pids = Set(procs.compactMap { $0["pid"] as? Int })
        let cpuA = a["cpu_ns"] as? UInt64 ?? 0
        let cpuB = b["cpu_ns"] as? UInt64 ?? 0
        guard pids.contains(Int(getpid())), pids.contains(Int(child.processIdentifier)),
              (a["phys_footprint"] as? UInt64 ?? 0) > 1_000_000,
              (a["threads"] as? Int ?? 0) >= 2, (a["open_fds"] as? Int ?? 0) >= 3,
              cpuB > cpuA, x > 0 else {
            fail("self-test: tree \(pids), footprint \(a["phys_footprint"] ?? 0), threads \(a["threads"] ?? 0), fds \(a["open_fds"] ?? 0), cpu \(cpuA) -> \(cpuB)")
        }
        print("tree-sample self-test OK -- itself and its child found, footprint \((a["phys_footprint"] as? UInt64 ?? 0) / 1024) KiB, CPU time grew \((cpuB - cpuA) / 1_000_000) ms")
        exit(0)
    }
}
