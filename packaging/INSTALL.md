# TelevyBackup macOS installation

1. Download the DMG or native tool archive matching the Mac architecture.
2. Download `SHA256SUMS` and verify the file before opening it:

   ```sh
   shasum -a 256 -c SHA256SUMS
   ```

3. The controlled distribution is ad-hoc signed. macOS may show a Gatekeeper warning. After verifying the checksum, open the app from Finder and use **Open** in the confirmation dialog. Do not remove quarantine before checksum verification.
4. The tool archive contains `televybackup`, `televybackupd`, `televybackup-mtproto-helper`, and a LaunchAgent template. Install the managed service explicitly with `televybackup daemon install-service`; uninstalling the service does not remove configuration or backup data.

## Strict APFS snapshots

The package also contains `TelevyBackup Snapshot Access.app` and the separately
installed root mount-helper artifact. Install the Access app at a user-selected
stable path, then run:

```sh
televybackup snapshot-access install --app "/path/to/TelevyBackup Snapshot Access.app"
```

The command creates the user LaunchAgent and performs the one-time administrator
transaction for the mount helper. Grant Full Disk Access manually to the exact
Access App and helper paths shown by `televybackup snapshot-access status`.
Normal and scheduled backups do not ask for a password. Strict mode remains
disabled until an explicit time-point verification succeeds.

Developer ID signing, notarization, automatic updates, and Homebrew formula updates are not part of this distribution.
