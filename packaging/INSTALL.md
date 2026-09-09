# TelevyBackup macOS installation

1. Download the DMG or native tool archive matching the Mac architecture.
2. Download `SHA256SUMS` and verify the file before opening it:

   ```sh
   shasum -a 256 -c SHA256SUMS
   ```

3. The controlled distribution is ad-hoc signed. macOS may show a Gatekeeper warning. After verifying the checksum, open the app from Finder and use **Open** in the confirmation dialog. Do not remove quarantine before checksum verification.
4. The tool archive contains `televybackup`, `televybackupd`, `televybackup-mtproto-helper`, the separate `TelevyBackup Snapshot Access.app`, and the narrowly-scoped `televybackup-snapshot-mount-helper`. Install the user services with `televybackup daemon install-service` and `televybackup snapshot-access install --app <path>`.
5. Strict APFS snapshot backups additionally require the following one-time setup:
   - Install the mount helper from an administrator-authorized shell with `sudo televybackup snapshot-mount-helper install`.
   - In System Settings, grant Full Disk Access to both the exact installed `TelevyBackup Snapshot Access.app` path and `/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-mount-helper`.
   - After either component's code identity or path changes, grant FDA again to the new exact identity before enabling strict mode.

   GUI, CLI, and `televybackupd` remain non-root and do not need FDA. Scheduled backups do not authenticate. Uninstalling either service does not remove configuration or backup data.

Developer ID signing, notarization, automatic updates, and Homebrew formula updates are not part of this distribution.
