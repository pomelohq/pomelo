import Foundation
import Darwin
import QuartzCore

// Captures the main thread's call stack the moment it hangs, so a stall can be attributed to a function instead of
// just a duration. A background watchdog watches a heartbeat the main thread bumps; when the heartbeat goes stale it
// briefly suspends the main thread, walks its frame-pointer chain into a fixed buffer (no allocation while
// suspended), resumes, then symbolicates and appends to /tmp/pom-hang.log. Apple Silicon (arm64) only.
final class MainThreadHangSampler {
    static let shared = MainThreadHangSampler()

    private var mainThread: thread_t = 0
    private var lastBeat = CACurrentMediaTime()
    private let lock = NSLock()
    private var running = false
    private let thresholdMs = 60.0
    private let logPath = "/tmp/pom-hang.log"

    // Call once on the main thread at launch.
    func registerMainThread() { mainThread = mach_thread_self() }

    // Call frequently on the main thread (e.g. from PerfHUD's 120 Hz timer).
    func beat() { lock.lock(); lastBeat = CACurrentMediaTime(); lock.unlock() }

    func start() {
        guard !running, mainThread != 0 else { return }
        running = true
        FileManager.default.createFile(atPath: logPath, contents: nil)
        Thread.detachNewThread { [weak self] in self?.watch() }
    }

    func stop() { running = false }

    private func watch() {
        while running {
            usleep(15_000)   // 15 ms
            lock.lock(); let last = lastBeat; lock.unlock()
            let gap = (CACurrentMediaTime() - last) * 1000
            if gap > thresholdMs {
                sampleMainThread(gapMs: gap)
                usleep(250_000)   // don't spam a single long hang
            }
        }
    }

    private func sampleMainThread(gapMs: Double) {
        guard thread_suspend(mainThread) == KERN_SUCCESS else { return }

        var frames = [UInt](repeating: 0, count: 64)
        var count = 0
        var state = arm_thread_state64_t()
        var stateCount = mach_msg_type_number_t(MemoryLayout<arm_thread_state64_t>.size / MemoryLayout<UInt32>.size)
        let ok = withUnsafeMutablePointer(to: &state) { ptr -> Bool in
            ptr.withMemoryRebound(to: natural_t.self, capacity: Int(stateCount)) { natural in
                thread_get_state(mainThread, ARM_THREAD_STATE64, natural, &stateCount) == KERN_SUCCESS
            }
        }
        if ok {
            // strip pointer authentication bits so addresses resolve
            frames[0] = UInt(state.__pc) & 0x0000_00ff_ffff_ffff
            count = 1
            var fp = UInt(state.__fp)
            while fp != 0, count < frames.count {
                guard let fpPtr = UnsafePointer<UInt>(bitPattern: fp) else { break }
                let nextFp = fpPtr.pointee
                let lr = (fpPtr + 1).pointee & 0x0000_00ff_ffff_ffff
                if lr == 0 { break }
                frames[count] = lr
                count += 1
                if nextFp <= fp { break }   // stack grows down; guard against loops
                fp = nextFp
            }
        }
        thread_resume(mainThread)

        guard count > 0 else { return }
        var lines: [String] = []
        for i in 0..<count {
            var info = Dl_info()
            if dladdr(UnsafeRawPointer(bitPattern: frames[i]), &info) != 0, let name = info.dli_sname {
                lines.append(String(cString: name))
            } else {
                lines.append(String(format: "0x%llx", UInt64(frames[i])))
            }
        }
        let ts = Self.fmt.string(from: Date())
        let body = lines.prefix(30).enumerated().map { "  \($0.offset)  \($0.element)" }.joined(separator: "\n")
        let entry = "\n[\(ts)] main hang \(Int(gapMs))ms\n\(body)\n"
        if let h = FileHandle(forWritingAtPath: logPath) {
            h.seekToEndOfFile(); h.write(Data(entry.utf8)); try? h.close()
        }
    }

    private static let fmt: DateFormatter = {
        let f = DateFormatter(); f.dateFormat = "HH:mm:ss.SSS"; return f
    }()
}
