import DCXLogicHelperCore
import SwiftUI

@main
struct DCXLogicHelperApp: App {
    @Environment(\.scenePhase) private var scenePhase
    @StateObject private var model = HelperAppModel()

    var body: some Scene {
        WindowGroup("DCX Logic Helper") {
            HelperContentView(model: model)
                .frame(minWidth: 640, minHeight: 650)
                .onAppear { model.enterForeground() }
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active:
                model.enterForeground()
            case .inactive:
                // Logic and the visible helper cannot both be key. Remaining
                // alive while inactive is still foreground-app operation.
                break
            case .background:
                model.leaveForeground()
            @unknown default:
                model.leaveForeground()
            }
        }
    }
}

@MainActor
final class HelperAppModel: ObservableObject {
    @Published private(set) var status = "Starting"
    @Published private(set) var configured = false
    @Published private(set) var midiOnline = false
    @Published private(set) var target = "Not configured"
    @Published private(set) var capabilities = "helper.status"
    @Published private(set) var recovery = "None"
    @Published var bindingID = ""
    @Published var ttyPath = "/dev/cu.usbserial-"
    @Published var expectedDeviceAddress = 0
    @Published var childTimeoutSeconds = Int(HelperConfigurationV1.defaultChildTimeoutSeconds)
    @Published private(set) var enabledFeatures: Set<HelperFeature> = [
        .identitySearch,
        .snapshotCapture,
    ]

    private let runtime: ForegroundHelperRuntime?

    init() {
        do {
            let runtime = try ForegroundHelperRuntime()
            self.runtime = runtime
            if let configuration = runtime.currentConfiguration() {
                populate(from: configuration)
            }
            status = "Ready to enter foreground"
        } catch {
            runtime = nil
            status = "App Group container unavailable"
        }
    }

    func enterForeground() {
        guard let runtime else { return }
        do {
            try runtime.enterForeground()
            if let configuration = runtime.currentConfiguration() {
                populate(from: configuration)
            }
            refresh()
            status = "Foreground bridge is running"
        } catch {
            status = "Foreground bridge failed to start"
            refresh()
        }
    }

    func leaveForeground() {
        runtime?.leaveForeground()
        refresh()
        status = "Bridge stopped while app is not active"
    }

    func refresh() {
        guard let status = runtime?.localStatus() else { return }
        configured = status.configured
        midiOnline = status.coreMIDI.online
        target = status.target.map {
            "\($0.bindingID), device \($0.expectedDeviceAddress)"
        } ?? "Not configured"
        capabilities = status.capabilities.map(\.rawValue).joined(separator: ", ")
        if status.recoveryUnavailable {
            recovery = "Unavailable (durable state is fail-closed)"
        } else if let active = status.recovery {
            recovery = "\(active.transactionID) · \(active.target.bindingID) · "
                + active.capabilities.map(\.rawValue).joined(separator: ", ")
        } else if let completion = status.completion {
            recovery = "Completed \(completion.transactionID) · exact baseline verified"
        } else {
            recovery = "None"
        }
    }

    func isEnabled(_ feature: HelperFeature) -> Bool {
        enabledFeatures.contains(feature)
    }

    var mutationCapabilitiesEnabled: Bool {
        HelperFeature.mutationFeatures.isSubset(of: enabledFeatures)
    }

    func setEnabled(_ feature: HelperFeature, _ enabled: Bool) {
        if HelperFeature.mutationFeatures.contains(feature) {
            setMutationCapabilitiesEnabled(enabled)
            return
        }
        if enabled {
            enabledFeatures.insert(feature)
        } else {
            enabledFeatures.remove(feature)
        }
    }

    func setMutationCapabilitiesEnabled(_ enabled: Bool) {
        if enabled {
            enabledFeatures.formUnion(HelperFeature.mutationFeatures)
            childTimeoutSeconds = max(
                childTimeoutSeconds,
                Int(HelperConfigurationV1.minimumMutationChildTimeoutSeconds)
            )
        } else {
            enabledFeatures.subtract(HelperFeature.mutationFeatures)
        }
    }

    func saveConfiguration() {
        guard let runtime,
              let device = UInt8(exactly: expectedDeviceAddress),
              let timeout = UInt8(exactly: childTimeoutSeconds) else {
            status = "Configuration values are outside their bounded ranges"
            return
        }
        do {
            let configuration = try HelperConfigurationV1(
                bindingID: bindingID,
                expectedDeviceAddress: device,
                ttyPath: ttyPath,
                enabledFeatures: HelperFeature.allCases.filter(enabledFeatures.contains),
                childTimeoutSeconds: timeout
            )
            try runtime.saveConfiguration(configuration)
            populate(from: configuration)
            refresh()
            status = "Configuration saved atomically and reloaded"
        } catch HelperConfigurationError.recoveryConfigurationPinned {
            status = "Configuration is pinned until mutation recovery verifies the baseline"
        } catch {
            status = "Configuration was rejected; verify binding, tty, device, features, and timeout"
        }
    }

    func reloadConfiguration() {
        guard let runtime else { return }
        do {
            let configuration = try runtime.reloadConfiguration()
            populate(from: configuration)
            refresh()
            status = "Saved configuration reloaded"
        } catch HelperConfigurationError.recoveryConfigurationPinned {
            refresh()
            status = "Saved configuration cannot replace the pinned mutation recovery configuration"
        } catch {
            refresh()
            status = "Saved configuration is unavailable or invalid"
        }
    }

    private func populate(from configuration: HelperConfigurationV1) {
        bindingID = configuration.bindingID
        ttyPath = configuration.ttyPath
        expectedDeviceAddress = Int(configuration.expectedDeviceAddress)
        childTimeoutSeconds = Int(configuration.childTimeoutSeconds)
        enabledFeatures = Set(configuration.enabledFeatures)
    }
}

private struct HelperContentView: View {
    @ObservedObject var model: HelperAppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("DCX Logic Helper")
                .font(.title)
            Text(model.status)
                .font(.headline)
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 10) {
                GridRow { Text("Configuration"); Text(model.configured ? "Loaded" : "Unavailable") }
                GridRow { Text("Target"); Text(model.target) }
                GridRow { Text("CoreMIDI"); Text(model.midiOnline ? "Commands + Status online" : "Offline") }
                GridRow { Text("Operations"); Text(model.capabilities).textSelection(.enabled) }
                GridRow { Text("Mutation recovery"); Text(model.recovery).textSelection(.enabled) }
            }
            Divider()
            Text("Exact DCX binding")
                .font(.headline)
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 10) {
                GridRow {
                    Text("Binding ID")
                    TextField("studio-dcx2496", text: $model.bindingID)
                        .textFieldStyle(.roundedBorder)
                }
                GridRow {
                    Text("TTY callout")
                    TextField("/dev/cu.usbserial-…", text: $model.ttyPath)
                        .textFieldStyle(.roundedBorder)
                        .textSelection(.enabled)
                }
                GridRow {
                    Text("Device address")
                    Stepper(
                        "\(model.expectedDeviceAddress)",
                        value: $model.expectedDeviceAddress,
                        in: 0...15
                    )
                }
                GridRow {
                    Text("Expected identity")
                    Text("Behringer DCX2496 · 38400 8N1 · address \(model.expectedDeviceAddress)")
                        .textSelection(.enabled)
                }
                GridRow {
                    Text("Child timeout")
                    Text("\(model.childTimeoutSeconds) seconds (bounded)")
                }
            }
            Text("Enabled operations")
                .font(.headline)
            LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], alignment: .leading) {
                ForEach(HelperFeature.independentlyConfigurableCases) { feature in
                    Toggle(
                        feature.displayName,
                        isOn: Binding(
                            get: { model.isEnabled(feature) },
                            set: { model.setEnabled(feature, $0) }
                        )
                    )
                }
                Toggle(
                    "Apply + Readback + Rollback",
                    isOn: Binding(
                        get: { model.mutationCapabilitiesEnabled },
                        set: { model.setMutationCapabilitiesEnabled($0) }
                    )
                )
            }
            if model.mutationCapabilitiesEnabled {
                Text("Apply is enabled only with complete readback and rollback recovery. The helper uses a bounded \(HelperConfigurationV1.minimumMutationChildTimeoutSeconds)-second child deadline.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            HStack {
                Button("Save and Activate Configuration") { model.saveConfiguration() }
                    .buttonStyle(.borderedProminent)
                Button("Reload Saved Configuration") { model.reloadConfiguration() }
                Spacer()
                Button("Refresh Local Status") { model.refresh() }
            }
            Divider()
            Text("This app owns one bounded dcxctl transaction at a time. It does not run as a daemon, and incoming MIDI Commands have no device mapping in bridge v1.")
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding(24)
    }
}
