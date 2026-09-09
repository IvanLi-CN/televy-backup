# APFS strict snapshot backup

## Reusable conclusion

The useful proof is a time-point mutation test: write a random probe, acquire
and mount a snapshot, mutate the live probe, read through the broker, and assert
that the broker returns the pre-mutation bytes. Existing-file hashes alone do
not prove snapshot reads.

## Safe evidence

Run the verifier only against a configured test target and the final workspace
artifacts. The default output is sanitized assertions. Keep raw diagnostics
local and temporary, remove the probe and recorded snapshot after the assertion,
and publish no paths, UUIDs, mount points, names, bytes, or raw IPC transcript.

## Boundary

The Access App owns file bytes. The root helper owns only APFS mount lifecycle.
The daemon continues to own scheduling, Keychain, encryption, and upload. The
strict source adapter keeps logical paths in manifests and releases the lease
before the upload phase.
