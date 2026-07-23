# auto_backup

A small Windows CLI that archives configured directories into split, checksummed `tar.zst` archives and copies them to a destination.

## Language

**Source**:
A directory listed in `source.paths` that gets archived on each backup run.
_Avoid_: source directory, backup target

**Archive**:
The single `tar.zst` file produced from one Source before splitting.
_Avoid_: backup file

**Part**:
One `.partNNN` chunk of an Archive that exceeded `max_part_size_mb`, tracked with its own SHA-256 in the Manifest.
_Avoid_: chunk, split file

**Manifest**:
The `.manifest.toml` companion file recording an Archive's Parts, checksums, and Skipped Files.
_Avoid_: metadata file

**Skipped File**:
A file inside a Source that could not be read — even through a Shadow Copy — and was left out of the Archive, recorded in the Manifest's `skipped_files`.
_Avoid_: unreadable file, locked file

**Volume**:
The Windows drive (e.g. `C:\`) a Source lives on. Shadow Copies are created per Volume, not per Source, so multiple Sources on the same Volume share one.
_Avoid_: drive, disk

**Shadow Copy**:
A point-in-time VSS snapshot of a Volume that lets a Source be archived even while its files are open elsewhere. One Shadow Copy set is created per backup run and deleted once the run finishes.
_Avoid_: snapshot, VSS snapshot

**Direct Read**:
Reading a Source straight from the live filesystem instead of through a Shadow Copy — the fallback used when VSS is unavailable for that Volume (no admin rights, VSS service disabled, unsupported Volume).
_Avoid_: fallback read, live read
