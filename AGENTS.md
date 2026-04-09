# AGENTS.md

## macOS: "发行版/Release" identification (do NOT guess by `/Applications`)

This repo has two **app variants** that may exist both as an installed release app and as a local
build output. **Do not infer the variant from the app's install path (e.g. `/Applications`) or
from the app name.**

Canonical meaning used in troubleshooting:

- **Release / 发行版**: bundle id `com.ivan.televybackup` (prod variant)
- **Dev / 开发版**: bundle id `com.ivan.televybackup.dev` (dev variant)

### How to confirm what is currently running

Prefer checking the **bundle id** of the running process:

```bash
# Find TelevyBackup PIDs and their bundle paths
lsappinfo list | rg -n '"TelevyBackup"'

# Confirm the bundle id (Release vs Dev) by PID
lsappinfo info -only bundleid -pid <PID>

# Optional: confirm executable path
ps -p <PID> -o command=
```

### How to confirm an `.app` bundle on disk

```bash
/usr/bin/mdls -name kMDItemCFBundleIdentifier -name kMDItemVersion "<path-to-app>.app"
```

### Logs/config directory note

Log location depends on the **config dir**:

- Keychain enabled (prod-like): default config dir is `~/Library/Application Support/TelevyBackup`
- Keychain disabled (dev default): `scripts/macos/run-app.sh` defaults to workspace
  `.dev/televybackup/config` and `.dev/televybackup/data`

When debugging "发行版" issues, always confirm the actual `--config-dir` / `TELEVYBACKUP_CONFIG_DIR`
used by the running instance before assuming a log path.

