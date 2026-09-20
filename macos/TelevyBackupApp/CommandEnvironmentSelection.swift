enum CommandEnvironmentSelection {
    static func resolve(
        applyProductEnvironment: Bool,
        processEnvironment: [String: String],
        productEnvironment: () -> [String: String]
    ) -> [String: String] {
        guard applyProductEnvironment else { return processEnvironment }
        return productEnvironment()
    }
}
