# Snapshot Browse Control IPC Contract

All methods use the existing authenticated local daemon control socket. The control socket, not the WebDAV listener, is the authority that creates and releases Snapshot Browse Sessions.

## `snapshot.browse.mount`

Creates a Snapshot Browse Session for one Backup Target.

```json
{
  "method": "snapshot.browse.mount",
  "params": {
    "targetId": "<target-id>",
    "allowCachedCatalog": false
  }
}
```

- The daemon first attempts remote catalog refresh.
- On refresh failure with a usable local catalog and `allow_cached_catalog: false`, it returns the typed error `snapshot.browse.catalog_refresh_unavailable` without creating a session.
- Telegram private-chat targets do not have the pinned bootstrap catalog required for remote-first refresh. They return the same typed error and require an explicit cached-catalog confirmation; this prevents a local-only catalog from being mislabeled `fresh`.
- Retrying the same target with `allow_cached_catalog: true` creates a cached-catalog session only when the operator has confirmed the fallback in the macOS client.
- A successful result contains a session ID, display volume name, mount descriptor, and `catalogSource: fresh|cached`. The mount descriptor contains the capability URL and is secret-bearing; the client keeps it in memory only and never places it in status UI or logs. Daemon request decoding also accepts the snake_case spellings `target_id` and `allow_cached_catalog` for non-Swift clients.
- The daemon prepares missing retained filemaps through the existing remote-first index path before reporting `fresh`. Cached mode skips that preparation and uses the retained local catalog and filemaps only.

## `snapshot.browse.status`

Returns a session's non-secret state: selected target ID, display volume name, catalog source, mount state, and whether Finder metadata overlay entries exist. It never returns the capability URL.

## `snapshot.browse.unmount`

Releases a specified session after Finder eject or explicit application unmount. Releasing a session revokes its capability, clears its memory overlay, stops the loopback listener, and removes cache pinning held exclusively by the active request.

## Recovery

On application startup or daemon recovery, the control plane releases every session and mount inherited from an earlier application or daemon instance before admitting a replacement session. A healthy Finder mount is not carried across that recovery boundary.

`snapshot.browse.recover` takes no parameters and returns `{ "recovered": true }`. It is idempotent and never returns a capability or credential.
