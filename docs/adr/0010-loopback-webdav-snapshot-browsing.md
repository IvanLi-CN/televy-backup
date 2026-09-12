# Loopback WebDAV Snapshot Browsing

TelevyBackup exposes a Finder-visible, read-only Backup Target Browse Volume through a daemon-owned WebDAV service bound only to `127.0.0.1`. A Snapshot Browse Session uses an OS-assigned port and an in-memory 256-bit capability path; it is mounted by the macOS client and never becomes a LAN or Internet service. This chooses native Finder WebDAV mounting over a macFUSE/FSKit filesystem because it avoids a system extension, developer signing, and reduced-security startup requirements while keeping remote-storage reads inside TelevyBackup.

## Considered Options

- A macFUSE/FSKit filesystem would offer a filesystem-native callback boundary, but the validated backend path is not sufficiently reliable on the supported macOS configuration and adds a system-extension dependency.
- A public or LAN WebDAV endpoint would require durable network authentication, TLS, discovery, and a materially larger attack surface.
- HTTP Basic authentication on loopback is rejected because `mount_webdav -s` requires HTTPS. Locally trusted HTTPS would introduce certificate installation and trust management without improving the same-user security boundary.

## Consequences

- The capability URL is a secret: it is never persisted, logged, shown in normal UI, or returned by status APIs.
- macOS `mount_webdav` accepts the URL only as a launch argument. The client uses `-S` and the URL is present only for the short-lived mount invocation; this transient process-argument visibility is an explicit macOS platform boundary, not a persisted or application-controlled disclosure. The daemon still binds only loopback and revokes the capability on unmount.
- The service can use ordinary HTTP only because it is constrained to loopback and authenticated by the unguessable capability path. It is not a model for remote WebDAV deployment.
- The daemon must own content resolution, encrypted-object caching, retention checks, and service cleanup; the macOS client only creates or releases the Finder mount through its authenticated local control boundary.
