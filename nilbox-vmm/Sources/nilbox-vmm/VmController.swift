
import Foundation
import Virtualization

/// Manages a single VZVirtualMachine instance.
/// All public methods must be called from the main queue.
class VmController: NSObject, VZVirtualMachineDelegate, VZVirtioSocketListenerDelegate {

    private var vm: VZVirtualMachine?
    private var relaySocketPath: String = ""
    private var relayToken: String = ""

    // MARK: - Configuration

    /// Configure and validate the VZVirtualMachineConfiguration.
    func configure(config: VmStartConfig) throws {
        #if arch(arm64)
        fputs("[VMM] Architecture: arm64\n", stderr)
        #elseif arch(x86_64)
        fputs("[VMM] Architecture: x86_64\n", stderr)
        #else
        fputs("[VMM] Architecture: unknown\n", stderr)
        #endif
        fputs("[VMM] macOS: \(ProcessInfo.processInfo.operatingSystemVersionString)\n", stderr)

        fputs("[VMM] config.disk_image = \(config.disk_image)\n", stderr)
        fputs("[VMM] config.kernel     = \(config.kernel ?? "(nil)")\n", stderr)
        fputs("[VMM] config.initrd     = \(config.initrd ?? "(nil)")\n", stderr)
        fputs("[VMM] config.append     = \(config.append ?? "(nil)")\n", stderr)
        fputs("[VMM] config.memory_mb  = \(config.memory_mb)\n", stderr)
        fputs("[VMM] config.cpus       = \(config.cpus)\n", stderr)
        fputs("[VMM] config.relay_socket = \(config.relay_socket)\n", stderr)

        let fm = FileManager.default
        if !fm.fileExists(atPath: config.disk_image) {
            throw NSError(domain: "nilbox-vmm", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: "disk_image not found: \(config.disk_image)"])
        }
        if let k = config.kernel, !k.isEmpty, !fm.fileExists(atPath: k) {
            throw NSError(domain: "nilbox-vmm", code: 3,
                          userInfo: [NSLocalizedDescriptionKey: "kernel not found: \(k)"])
        }
        if let i = config.initrd, !i.isEmpty, !fm.fileExists(atPath: i) {
            throw NSError(domain: "nilbox-vmm", code: 4,
                          userInfo: [NSLocalizedDescriptionKey: "initrd not found: \(i)"])
        }

        let vmConfig = VZVirtualMachineConfiguration()

        // --- Boot loader ---
        guard let kernelPath = config.kernel, !kernelPath.isEmpty else {
            throw NSError(domain: "nilbox-vmm", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "kernel path is required"])
        }
        let bootLoader = VZLinuxBootLoader(kernelURL: URL(fileURLWithPath: kernelPath))
        if let initrd = config.initrd, !initrd.isEmpty {
            bootLoader.initialRamdiskURL = URL(fileURLWithPath: initrd)
        }
        if let append = config.append, !append.isEmpty {
            bootLoader.commandLine = append
        }
        vmConfig.bootLoader = bootLoader

        // --- CPU & Memory ---
        vmConfig.cpuCount = max(1, config.cpus)
        vmConfig.memorySize = UInt64(config.memory_mb) * 1024 * 1024

        // --- Block storage (raw format required by VZDiskImageStorageDeviceAttachment) ---
        let diskURL = URL(fileURLWithPath: config.disk_image)
        fputs("[VMM] Opening disk: \(diskURL.path)\n", stderr)
        let diskAttachment = try VZDiskImageStorageDeviceAttachment(url: diskURL, readOnly: false)
        let disk = VZVirtioBlockDeviceConfiguration(attachment: diskAttachment)
        vmConfig.storageDevices = [disk]

        // --- No network interface (vsock-only isolation) ---
        vmConfig.networkDevices = []

        // --- VZVirtioSocketDevice (native vsock) ---
        vmConfig.socketDevices = [VZVirtioSocketDeviceConfiguration()]

        // --- Entropy ---
        vmConfig.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]

        // --- Memory balloon ---
        vmConfig.memoryBalloonDevices = [VZVirtioTraditionalMemoryBalloonDeviceConfiguration()]

        // --- Serial console (virtio console → hvc0 in guest) ---
        let consolePort = VZVirtioConsoleDeviceSerialPortConfiguration()
        consolePort.attachment = VZFileHandleSerialPortAttachment(
            fileHandleForReading: FileHandle(forReadingAtPath: "/dev/null")!,
            fileHandleForWriting: FileHandle.standardError
        )
        vmConfig.serialPorts = [consolePort]

        // Validate before creating VM
        try vmConfig.validate()

        self.relaySocketPath = config.relay_socket
        self.relayToken = config.relay_token ?? ""

        let machine = VZVirtualMachine(configuration: vmConfig)
        machine.delegate = self

        // Register socket listener on the runtime device for port 18088 (OUTBOUND_PORT)
        if let socketDevice = machine.socketDevices.first as? VZVirtioSocketDevice {
            let socketListener = VZVirtioSocketListener()
            socketListener.delegate = self
            socketDevice.setSocketListener(socketListener, forPort: 18088)
        } else {
            fputs("[VMM] Warning: VZVirtioSocketDevice not found after VM creation\n", stderr)
        }

        self.vm = machine
    }

    /// Guest agent vsock port (vm-agent listens here)
    private static let guestAgentPort: UInt32 = 1024

    // MARK: - Lifecycle

    func startVM() {
        guard let vm = vm else {
            printEvent(["event": "error", "message": "VM not configured"])
            return
        }
        vm.start { [weak self] result in
            switch result {
            case .success:
                printEvent(["event": "started"])
                // Initiate vsock connection to the guest vm-agent
                self?.connectToGuest()
            case .failure(let error):
                printEvent(["event": "error", "message": error.localizedDescription])
            }
        }
    }

    /// Connect from host to guest vm-agent via vsock, then relay to the Unix socket.
    private func connectToGuest() {
        guard let vm = vm,
              let socketDevice = vm.socketDevices.first as? VZVirtioSocketDevice else {
            fputs("[VMM] connectToGuest: no socket device available\n", stderr)
            return
        }
        connectWithRetry(socketDevice: socketDevice, port: Self.guestAgentPort, attempt: 1, maxAttempts: 30)
    }

    private func connectWithRetry(socketDevice: VZVirtioSocketDevice, port: UInt32, attempt: Int, maxAttempts: Int) {
        fputs("[VMM] Connecting to guest port \(port) (attempt \(attempt)/\(maxAttempts))\n", stderr)

        socketDevice.connect(toPort: port) { [weak self] result in
            switch result {
            case .success(let connection):
                fputs("[VMM] Connected to guest vm-agent on port \(port)\n", stderr)
                guard let relayPath = self?.relaySocketPath, !relayPath.isEmpty else {
                    fputs("[VMM] relay socket path not set\n", stderr)
                    return
                }
                VsockRelay.relay(connection: connection, port: port, relayPath: relayPath, relayToken: self?.relayToken ?? "")

            case .failure(let error):
                fputs("[VMM] Connect to guest port \(port) failed (attempt \(attempt)): \(error.localizedDescription)\n", stderr)
                if attempt < maxAttempts {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {
                        self?.connectWithRetry(socketDevice: socketDevice, port: port, attempt: attempt + 1, maxAttempts: maxAttempts)
                    }
                } else {
                    fputs("[VMM] Failed to connect to guest after \(maxAttempts) attempts\n", stderr)
                }
            }
        }
    }

    func stopVM() {
        guard let vm = vm else { return }
        if vm.canRequestStop {
            do {
                try vm.requestStop()
                return
            } catch {
                fputs("[VMM] requestStop failed: \(error.localizedDescription)\n", stderr)
            }
        }
        vm.stop { _ in }
    }

    // MARK: - VZVirtualMachineDelegate

    func guestDidStop(_ virtualMachine: VZVirtualMachine) {
        printEvent(["event": "stopped"])
        exit(0)
    }

    func virtualMachine(_ virtualMachine: VZVirtualMachine,
                        didStopWithError error: Error) {
        printEvent(["event": "error", "message": error.localizedDescription])
        exit(1)
    }

    // MARK: - VZVirtioSocketListenerDelegate

    func listener(_ listener: VZVirtioSocketListener,
                  shouldAcceptNewConnection connection: VZVirtioSocketConnection,
                  from socketDevice: VZVirtioSocketDevice) -> Bool {
        let port = connection.destinationPort
        fputs("[VMM] Guest vsock connection to port \(port)\n", stderr)
        VsockRelay.relay(connection: connection,
                         port: port,
                         relayPath: relaySocketPath,
                         relayToken: relayToken)
        return true
    }
}
