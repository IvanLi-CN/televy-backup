# TelevyBackup macOS installation

1. Download the DMG or native tool archive matching the Mac architecture.
2. Download `SHA256SUMS` and verify the file before opening it:

   ```sh
   shasum -a 256 -c SHA256SUMS
   ```

3. The controlled distribution is ad-hoc signed. macOS may show a Gatekeeper warning. After verifying the checksum, open the app from Finder and use **Open** in the confirmation dialog. Do not remove quarantine before checksum verification.
4. The tool archive contains `televybackup`, `televybackupd`, `televybackup-mtproto-helper`, the separate `TelevyBackup Snapshot Access.app`, and the narrowly-scoped `televybackup-snapshot-mount-helper`. Install the user services with `televybackup daemon install-service` and `televybackup snapshot-access install --app <path>`. Install the mount helper once from an administrator-authorized shell with `sudo televybackup snapshot-mount-helper install`; this is only needed for strict APFS snapshot backups. Grant Full Disk Access to the exact Snapshot Access app path and, when macOS lists it separately, the installed mount-helper path. Scheduled backups do not authenticate. Uninstalling either service does not remove configuration or backup data.

Developer ID signing, notarization, automatic updates, and Homebrew formula updates are not part of this distribution.
