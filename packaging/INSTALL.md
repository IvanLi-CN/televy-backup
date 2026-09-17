# TelevyBackup macOS installation

1. Download the DMG or native tool archive matching the Mac architecture.
2. Download `SHA256SUMS` and verify the file before opening it:

   ```sh
   shasum -a 256 -c SHA256SUMS
   ```

3. The controlled distribution is ad-hoc signed. macOS may show a Gatekeeper warning. After verifying the checksum, open the app from Finder and use **Open** in the confirmation dialog. Do not remove quarantine before checksum verification.
4. In the DMG, drag the single `TelevyBackup.app` icon to `Applications`. The `Applications` icon is an alias for `/Applications`; the private Snapshot Access helper remains nested inside the app and is not a separate installable item.
5. The tool archive contains `televybackup`, `televybackupd`, `televybackup-mtproto-helper`, and the narrowly-scoped `televybackup-snapshot-mount-helper`. Snapshot Access is private to the DMG's single `TelevyBackup.app`; it is not installed from the tools archive. The ad-hoc release registers its embedded LaunchAgent automatically with `launchctl` using the bundle-relative `BundleProgram`.
6. Strict APFS snapshot backups additionally require the following one-time setup:
   - Install the mount helper from an administrator-authorized shell with `sudo televybackup snapshot-mount-helper install`.
   - On the first launch after the layout migration, grant Full Disk Access to the exact embedded helper path shown in Settings and `/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-mount-helper`.
   - Ordinary main-app updates reuse the unchanged Snapshot Access helper and do not request FDA again. If Snapshot Access code or FDA behavior changes, grant FDA again to the new exact helper identity. The root mount helper is not automatically updated by a main-app release.

   GUI, CLI, and `televybackupd` remain non-root and do not need FDA. Scheduled backups do not authenticate. Removing a managed service does not remove configuration or backup data.

Developer ID signing, notarization, automatic updates, and Homebrew formula updates are not part of this distribution.

## Homebrew GUI app

The Cask downloads the latest stable Universal 2 DMG and verifies it against its SHA-256 before installing the app:

```sh
brew tap IvanLi-CN/televy-backup https://github.com/IvanLi-CN/televy-backup.git
brew install --cask IvanLi-CN/televy-backup/televybackup
```

Homebrew leaves quarantine intact. If macOS blocks the first launch, use Finder to open the app and approve it through Gatekeeper (for example, **System Settings > Privacy & Security > Open Anyway**). Do not remove quarantine. The Cask installs only `TelevyBackup.app`; it does not install or configure the daemon, mount helper, FDA permission, configuration, or backup data.
