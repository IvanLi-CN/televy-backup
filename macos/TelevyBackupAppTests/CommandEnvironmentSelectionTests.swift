import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

@main
enum CommandEnvironmentSelectionTestsMain {
    static func main() {
        let inherited = ["PATH": "/usr/bin", "TELEVYBACKUP_CONFIG_DIR": "/tmp/inherited"]
        var productEnvironmentBuildCount = 0
        let signatureProbeEnvironment = CommandEnvironmentSelection.resolve(
            applyProductEnvironment: false,
            processEnvironment: inherited
        ) {
            productEnvironmentBuildCount += 1
            return ["TELEVYBACKUP_CONFIG_DIR": "/tmp/product"]
        }

        expect(signatureProbeEnvironment == inherited, "signature probe should inherit the process environment")
        expect(productEnvironmentBuildCount == 0, "signature probe must not recursively build the product environment")

        let productEnvironment = CommandEnvironmentSelection.resolve(
            applyProductEnvironment: true,
            processEnvironment: inherited
        ) {
            productEnvironmentBuildCount += 1
            return ["TELEVYBACKUP_CONFIG_DIR": "/tmp/product"]
        }

        expect(productEnvironment == ["TELEVYBACKUP_CONFIG_DIR": "/tmp/product"], "ordinary commands should use the product environment")
        expect(productEnvironmentBuildCount == 1, "ordinary command should build the product environment once")
        print("OK: CommandEnvironmentSelectionTests")
    }
}
