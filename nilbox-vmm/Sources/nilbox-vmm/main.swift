
import Foundation
import Virtualization

// MARK: - Control Protocol Types

struct VmStartConfig: Decodable {
    let disk_image: String
    let kernel: String?
    let initrd: String?
    let append: String?
    let memory_mb: Int
    let cpus: Int
    let relay_socket: String
    let relay_token: String?
}

struct IncomingCommand: Decodable {
    let cmd: String
    let config: VmStartConfig?
}

// MARK: - Helpers

func printEvent(_ dict: [String: String]) {
    if let data = try? JSONSerialization.data(withJSONObject: dict),
       let line = String(data: data, encoding: .utf8) {
        print(line)
        fflush(stdout)
    }
}

// MARK: - Main

let controller = VmController()

// Read stdin on background thread; dispatch VM ops to main queue
// (VZVirtualMachine requires main queue)
DispatchQueue.global(qos: .userInitiated).async {
    while let line = readLine(strippingNewline: true) {
        guard !line.isEmpty,
              let data = line.data(using: .utf8),
              let cmd = try? JSONDecoder().decode(IncomingCommand.self, from: data)
        else { continue }

        DispatchQueue.main.async {
            switch cmd.cmd {
            case "start":
                guard let config = cmd.config else {
                    printEvent(["event": "error", "message": "start command missing config"])
                    return
                }
                do {
                    try controller.configure(config: config)
                    controller.startVM()
                } catch {
                    printEvent(["event": "error", "message": error.localizedDescription])
                }
            case "stop":
                controller.stopVM()
            default:
                break
            }
        }
    }
    // stdin closed — request graceful shutdown
    DispatchQueue.main.async {
        controller.stopVM()
    }
}

// Signal that the VMM process is ready to receive commands
printEvent(["event": "ready"])

// Keep the main run loop alive for VZVirtualMachine callbacks
RunLoop.main.run()
