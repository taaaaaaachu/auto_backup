//! Windows VSS (Volume Shadow Copy Service) integration.
//!
//! Backup sources are read through a VSS shadow copy so that files locked by
//! another process (Outlook's `.pst`, an open Office document, ...) can
//! still be archived. One shadow copy set is created per backup run,
//! covering every distinct volume touched by `source.paths`, and is deleted
//! again once the run finishes. If VSS is unavailable for a volume (no
//! admin rights, VSS service disabled, unsupported volume, ...) that volume
//! falls back to reading the live filesystem directly.

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr;

use anyhow::{ensure, Result};
use winapi::shared::guiddef::GUID;
use winapi::shared::minwindef::TRUE;
use winapi::um::combaseapi::{CoInitializeEx, CoUninitialize};
use winapi::um::objbase::COINIT_MULTITHREADED;
use winapi::um::vsbackup::{
    CreateVssBackupComponents, IVssBackupComponents, VssFreeSnapshotProperties,
};
use winapi::um::vss::{
    IVssAsync, VSS_CTX_CLIENT_ACCESSIBLE, VSS_ID, VSS_OBJECT_SNAPSHOT_SET, VSS_SNAPSHOT_PROP,
};
use winapi::um::winbase::INFINITE;
use winapi::um::winnt::{HRESULT, LONG};
use wio::com::ComPtr;

/// Where to read a backup source from, decided by [`VssSession::resolve`].
pub struct SourceRead {
    pub read_path: PathBuf,
    pub used_vss: bool,
}

enum VolumeState {
    Snapshotted { device_object: PathBuf },
    Unavailable,
}

/// Coordinates the single VSS snapshot set for one backup run.
pub struct VssSession {
    backup_components: Option<ComPtr<IVssBackupComponents>>,
    snapshot_set_id: Option<VSS_ID>,
    volumes: HashMap<PathBuf, VolumeState>,
    com_initialized: bool,
}

impl VssSession {
    /// Attempts to create shadow copies for every distinct volume backing
    /// `sources`. Never fails: on any error the session falls back to direct
    /// reads. Returns the session plus human-readable log lines describing
    /// the outcome per volume.
    pub fn new(sources: &[PathBuf]) -> (Self, Vec<String>) {
        let mut session = VssSession {
            backup_components: None,
            snapshot_set_id: None,
            volumes: HashMap::new(),
            com_initialized: false,
        };
        let mut logs = Vec::new();

        let mut roots = Vec::new();
        for source in sources {
            match volume_root(source) {
                Ok(root) => {
                    if !roots.contains(&root) {
                        roots.push(root);
                    }
                }
                Err(reason) => logs.push(format!(
                    "VSS unavailable for {}: {reason}; falling back to direct read",
                    source.display()
                )),
            }
        }

        if roots.is_empty() {
            return (session, logs);
        }

        match session.create_snapshot_set(&roots) {
            Ok(device_objects) => {
                for root in &roots {
                    match device_objects.get(root) {
                        Some(device_object) => {
                            logs.push(format!(
                                "VSS snapshot created for volume {}",
                                root.display()
                            ));
                            session.volumes.insert(
                                root.clone(),
                                VolumeState::Snapshotted {
                                    device_object: device_object.clone(),
                                },
                            );
                        }
                        None => {
                            logs.push(format!(
                                "VSS unavailable for volume {}: rejected by the VSS provider; falling back to direct read",
                                root.display()
                            ));
                            session
                                .volumes
                                .insert(root.clone(), VolumeState::Unavailable);
                        }
                    }
                }
            }
            Err(error) => {
                logs.push(format!(
                    "VSS unavailable: {error}; falling back to direct read for all sources"
                ));
                for root in roots {
                    session.volumes.insert(root, VolumeState::Unavailable);
                }
            }
        }

        (session, logs)
    }

    /// Decides where to read `source` from: inside the shadow copy if one
    /// covers its volume, otherwise the original path unchanged.
    pub fn resolve(&self, source: &Path) -> SourceRead {
        let fallback = || SourceRead {
            read_path: source.to_path_buf(),
            used_vss: false,
        };

        let Ok(root) = volume_root(source) else {
            return fallback();
        };
        match self.volumes.get(&root) {
            Some(VolumeState::Snapshotted { device_object }) => match source.strip_prefix(&root) {
                Ok(relative) => SourceRead {
                    read_path: device_object.join(relative),
                    used_vss: true,
                },
                Err(_) => fallback(),
            },
            _ => fallback(),
        }
    }

    /// Creates one snapshot set covering `roots`. Individual volumes that
    /// the VSS provider rejects are simply absent from the returned map; a
    /// systemic failure (no admin rights, VSS service disabled, ...) fails
    /// the whole set and returns `Err`.
    fn create_snapshot_set(&mut self, roots: &[PathBuf]) -> Result<HashMap<PathBuf, PathBuf>> {
        unsafe {
            let hr = CoInitializeEx(ptr::null_mut(), COINIT_MULTITHREADED);
            ensure!(hr >= 0, "CoInitializeEx failed (0x{:08X})", hr as u32);
            self.com_initialized = true;

            let mut raw: *mut IVssBackupComponents = ptr::null_mut();
            let hr = CreateVssBackupComponents(&mut raw);
            ensure!(
                hr >= 0 && !raw.is_null(),
                "CreateVssBackupComponents failed (0x{:08X})",
                hr as u32
            );
            let vss = ComPtr::from_raw(raw);

            let hr = vss.InitializeForBackup(ptr::null_mut());
            ensure!(hr >= 0, "InitializeForBackup failed (0x{:08X})", hr as u32);

            let hr = vss.SetContext(VSS_CTX_CLIENT_ACCESSIBLE as LONG);
            ensure!(hr >= 0, "SetContext failed (0x{:08X})", hr as u32);

            let mut set_id: VSS_ID = std::mem::zeroed();
            let hr = vss.StartSnapshotSet(&mut set_id);
            ensure!(hr >= 0, "StartSnapshotSet failed (0x{:08X})", hr as u32);

            let mut snapshot_ids: HashMap<PathBuf, VSS_ID> = HashMap::new();
            for root in roots {
                let mut wide_volume = to_wide_null(root);
                let provider_id: GUID = std::mem::zeroed();
                let mut snapshot_id: VSS_ID = std::mem::zeroed();
                let hr =
                    vss.AddToSnapshotSet(wide_volume.as_mut_ptr(), provider_id, &mut snapshot_id);
                if hr >= 0 {
                    snapshot_ids.insert(root.clone(), snapshot_id);
                }
            }
            ensure!(
                !snapshot_ids.is_empty(),
                "no requested volume could be added to the VSS snapshot set"
            );

            // VSS_CTX_CLIENT_ACCESSIBLE does not involve writers, and calling
            // PrepareForBackup/AbortBackup/BackupComplete under a
            // non-writer-participating context is documented to return an
            // error, so this goes straight from AddToSnapshotSet to
            // DoSnapshotSet. See ADR-0003.
            let mut async_ptr: *mut IVssAsync = ptr::null_mut();
            let hr = vss.DoSnapshotSet(&mut async_ptr);
            ensure!(
                hr >= 0 && !async_ptr.is_null(),
                "DoSnapshotSet failed (0x{:08X})",
                hr as u32
            );
            wait_for_async(async_ptr, "DoSnapshotSet")?;

            let mut device_objects = HashMap::new();
            for (root, snapshot_id) in &snapshot_ids {
                let mut prop: VSS_SNAPSHOT_PROP = std::mem::zeroed();
                let hr = vss.GetSnapshotProperties(*snapshot_id, &mut prop);
                if hr >= 0 {
                    if let Some(device_object) =
                        wide_ptr_to_pathbuf(prop.m_pwszSnapshotDeviceObject)
                    {
                        device_objects.insert(root.clone(), device_object);
                    }
                    VssFreeSnapshotProperties(&mut prop);
                }
            }

            self.backup_components = Some(vss);
            self.snapshot_set_id = Some(set_id);
            Ok(device_objects)
        }
    }
}

impl Drop for VssSession {
    fn drop(&mut self) {
        if let (Some(vss), Some(set_id)) = (self.backup_components.as_ref(), self.snapshot_set_id) {
            let mut deleted: LONG = 0;
            let mut nondeleted: VSS_ID = unsafe { std::mem::zeroed() };
            unsafe {
                let _ = vss.DeleteSnapshots(
                    set_id,
                    VSS_OBJECT_SNAPSHOT_SET,
                    TRUE,
                    &mut deleted,
                    &mut nondeleted,
                );
            }
        }
        // Release the COM object before uninitializing COM on this thread.
        self.backup_components.take();
        if self.com_initialized {
            unsafe { CoUninitialize() };
        }
    }
}

unsafe fn wait_for_async(async_ptr: *mut IVssAsync, step: &str) -> Result<()> {
    let async_op = ComPtr::from_raw(async_ptr);
    let hr = async_op.Wait(INFINITE);
    ensure!(hr >= 0, "{step} wait failed (0x{:08X})", hr as u32);

    let mut result: HRESULT = 0;
    let hr = async_op.QueryStatus(&mut result, ptr::null_mut());
    ensure!(hr >= 0, "{step} QueryStatus failed (0x{:08X})", hr as u32);
    ensure!(
        result >= 0,
        "{step} completed with failure (0x{:08X})",
        result as u32
    );
    Ok(())
}

/// Returns the drive-letter volume root (e.g. `C:\`) that `path` lives on.
/// UNC paths, relative paths, and other prefixes are not supported by this
/// simple client and are reported as an error so the caller can fall back.
fn volume_root(path: &Path) -> std::result::Result<PathBuf, String> {
    match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                let drive = (letter as char).to_ascii_uppercase();
                Ok(PathBuf::from(format!("{drive}:\\")))
            }
            _ => Err(
                "path is not on a local drive letter (UNC/network paths are unsupported)"
                    .to_string(),
            ),
        },
        _ => Err("path has no drive letter".to_string()),
    }
}

fn to_wide_null(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn wide_ptr_to_pathbuf(ptr: *const u16) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    Some(PathBuf::from(OsString::from_wide(slice)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_root_extracts_drive_letter_uppercase() {
        assert_eq!(
            volume_root(Path::new("c:/Users/name/Documents")).unwrap(),
            PathBuf::from("C:\\")
        );
        assert_eq!(
            volume_root(Path::new(r"D:\backups")).unwrap(),
            PathBuf::from("D:\\")
        );
    }

    #[test]
    fn volume_root_rejects_relative_and_unc_paths() {
        assert!(volume_root(Path::new("relative/path")).is_err());
        assert!(volume_root(Path::new(r"\\server\share\path")).is_err());
    }

    #[test]
    fn resolve_falls_back_when_no_session_covers_the_volume() {
        let (session, _logs) = VssSession::new(&[]);
        let read = session.resolve(Path::new(r"C:\Users\name\Documents"));
        assert!(!read.used_vss);
        assert_eq!(read.read_path, PathBuf::from(r"C:\Users\name\Documents"));
    }
}
