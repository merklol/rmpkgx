//! macOS object-reference adapter. Legacy CoreServices FSRefs are opaque object
//! identities, not source path strings. Never fall back to a path-based move.
use super::native::Pinned;
use anyhow::{ensure, Context};
use std::{
    ffi::{CStr, CString},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

#[repr(C)]
#[derive(Clone, Copy)]
struct FsRef {
    opaque: [u8; 80],
}
#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn FSPathMakeRefWithOptions(
        path: *const u8,
        options: u32,
        reference: *mut FsRef,
        directory: *mut u8,
    ) -> i32;
    fn FSRefMakePath(reference: *const FsRef, path: *mut u8, capacity: u32) -> i32;
    fn FSCompareFSRefs(one: *const FsRef, two: *const FsRef) -> i16;
    fn FSMoveObjectToTrashSync(source: *const FsRef, target: *mut FsRef, options: u32) -> i32;
    fn FSMoveObjectSync(
        source: *const FsRef,
        directory: *const FsRef,
        name: *const std::ffi::c_void,
        target: *mut FsRef,
        options: u32,
    ) -> i32;
    #[cfg(test)]
    fn FSMoveObject(source: *const FsRef, destination: *const FsRef, target: *mut FsRef) -> i16;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithBytes(
        allocator: *const std::ffi::c_void,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> *const std::ffi::c_void;
    fn CFRelease(object: *const std::ffi::c_void);
}
struct NativeString(*const std::ffi::c_void);
impl Drop for NativeString {
    fn drop(&mut self) {
        // SAFETY: NativeString owns a non-null CFString returned by Create.
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}
pub struct Reference {
    reference: FsRef,
}
impl Reference {
    pub fn capture(path: &Path, pin: &Pinned) -> anyhow::Result<Self> {
        pin.revalidate()?;
        let name = CString::new(path.as_os_str().as_bytes())?;
        let mut reference = FsRef { opaque: [0; 80] };
        // SAFETY: valid NUL-terminated input and writable FSRef. Option 1 refuses
        // to resolve the final symlink; pinned validation rejects all symlinks.
        let status = unsafe {
            FSPathMakeRefWithOptions(
                name.as_ptr().cast(),
                1,
                &mut reference,
                std::ptr::null_mut(),
            )
        };
        ensure!(
            status == 0,
            "cannot capture macOS file reference: OSStatus {status}"
        );
        let result = Self { reference };
        pin.matches(&result.path()?)?;
        pin.revalidate()?;
        Ok(result)
    }
    fn path(&self) -> anyhow::Result<PathBuf> {
        let mut bytes = [0u8; 4096];
        // SAFETY: valid reference and writable bounded output buffer.
        let status =
            unsafe { FSRefMakePath(&self.reference, bytes.as_mut_ptr(), bytes.len() as u32) };
        ensure!(
            status == 0,
            "cannot resolve macOS file reference: OSStatus {status}"
        );
        let path = CStr::from_bytes_until_nul(&bytes).context("unterminated reference path")?;
        Ok(Path::new(std::ffi::OsStr::from_bytes(path.to_bytes())).to_owned())
    }
    fn verify_target(&self, target: &FsRef) -> anyhow::Result<()> {
        // SAFETY: both FSRefs are initialized and live for this call.
        let status = unsafe { FSCompareFSRefs(&self.reference, target) };
        ensure!(
            status == 0,
            "native reference comparison returned OSStatus {status}"
        );
        Ok(())
    }
    pub fn restore_to(
        &self,
        pin: &Pinned,
        parent: &Reference,
        parent_pin: &Pinned,
        destination: &Path,
    ) -> anyhow::Result<super::trash::MoveReceipt> {
        pin.revalidate()?;
        pin.matches(&self.path()?)?;
        parent_pin.revalidate_identity()?;
        parent_pin.matches(&parent.path()?)?;
        match std::fs::symlink_metadata(destination) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => anyhow::bail!(
                "destination exists or cannot be inspected; nothing will be overwritten"
            ),
        }
        let name = destination
            .file_name()
            .and_then(|s| s.to_str())
            .context("destination filename is not UTF-8")?;
        // SAFETY: UTF-8 input is live for the call; returned CF object is owned.
        let name = NativeString(unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                name.as_ptr(),
                name.len() as isize,
                0x08000100,
                0,
            )
        });
        ensure!(!name.0.is_null(), "cannot create destination name");
        let mut target = FsRef { opaque: [0; 80] };
        // SAFETY: valid references and owned CFString. No overwrite flag; cross-volume copy/delete prohibited.
        let status = unsafe {
            FSMoveObjectSync(
                &self.reference,
                &parent.reference,
                name.0,
                &mut target,
                0x04,
            )
        };
        ensure!(
            status == 0,
            "Restore failed: OSStatus {status}; no overwrite or privilege escalation attempted"
        );
        Ok(super::trash::MoveReceipt::after_success(
            Self { reference: target }.path(),
            self.verify_target(&target),
        ))
    }
    pub fn trash(&self, pin: &Pinned) -> anyhow::Result<super::trash::MoveReceipt> {
        pin.revalidate()?;
        let source = self.path()?;
        pin.matches(&source)?;
        let mut target = FsRef { opaque: [0; 80] };
        // SAFETY: valid source FSRef and writable target; 0x04 prohibits
        // cross-volume copy/delete. No overwrite or skip-errors flags enabled.
        let status = unsafe { FSMoveObjectToTrashSync(&self.reference, &mut target, 0x04) };
        ensure!(status == 0, "{}", trash_failure(status, &source));
        // Compare opaque object references without reopening the protected
        // Trash directory. Failure after OS success is never a failed move.
        let verification = self.verify_target(&target);
        let destination = Self { reference: target }.path();
        Ok(super::trash::MoveReceipt::after_success(
            destination,
            verification,
        ))
    }
}
fn trash_failure(status: i32, source: &Path) -> String {
    if status != -5000 {
        return format!("Trash operation failed: OSStatus {status}; no permanent-delete fallback");
    }
    let mut message = "Access denied: OSStatus -5000 (afpAccessDenied). macOS refused the Trash operation.\n  Reading a path does not establish permission to move it.\n  Possible causes include privacy restrictions, filesystem permissions, ACLs or file flags.".to_owned();
    if dirs::home_dir().is_some_and(|home| {
        source.starts_with(home.join("Library/Containers"))
            || source.starts_with(home.join("Library/Group Containers"))
    }) {
        message.push_str(
            "\n  This is an application container; macOS container protection may also apply.",
        );
    }
    message.push_str("\n  Review Privacy & Security permissions for the terminal/app hosting rmpkgx.\n  Full Disk Access is not a guaranteed fix; this error does not identify the exact cause.\n  No privilege escalation or permanent deletion was attempted.");
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn object_reference_moves_original_after_path_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let dest = root.join("destination");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(&source, "approved").unwrap();
        let pin = Pinned::capture(&source).unwrap();
        let reference = Reference::capture(&source, &pin).unwrap();
        let dest_pin = Pinned::capture(&dest).unwrap();
        let dest_ref = Reference::capture(&dest, &dest_pin).unwrap();
        std::fs::rename(&source, root.join("renamed")).unwrap();
        std::fs::write(&source, "replacement").unwrap();
        let mut moved = FsRef { opaque: [0; 80] };
        // SAFETY: all three references are initialized and remain live.
        let status = unsafe { FSMoveObject(&reference.reference, &dest_ref.reference, &mut moved) };
        assert_eq!(status, 0);
        reference.verify_target(&moved).unwrap();
        let moved = Reference { reference: moved }.path().unwrap();
        assert!(moved.starts_with(&dest));
        pin.matches(&moved).unwrap();
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "approved");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "replacement");
    }
    #[test]
    fn mismatched_identity_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let one = root.join("one");
        let two = root.join("two");
        std::fs::write(&one, "one").unwrap();
        std::fs::write(&two, "two").unwrap();
        let pin = Pinned::capture(&one).unwrap();
        assert!(Reference::capture(&two, &pin).is_err());
    }
    #[test]
    fn reference_move_collision_preserves_both_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("same");
        let destination = root.join("destination");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&source, "source").unwrap();
        std::fs::write(destination.join("same"), "existing").unwrap();
        let source_pin = Pinned::capture(&source).unwrap();
        let source_ref = Reference::capture(&source, &source_pin).unwrap();
        let dest_pin = Pinned::capture(&destination).unwrap();
        let dest_ref = Reference::capture(&destination, &dest_pin).unwrap();
        let mut target = FsRef { opaque: [0; 80] };
        // SAFETY: valid input references and a writable target buffer.
        let status =
            unsafe { FSMoveObject(&source_ref.reference, &dest_ref.reference, &mut target) };
        assert_ne!(status, 0);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "source");
        assert_eq!(
            std::fs::read_to_string(destination.join("same")).unwrap(),
            "existing"
        );
    }
    #[test]
    fn denied_trash_operation_has_actionable_but_nonprescriptive_diagnostics() {
        let message = trash_failure(-5000, Path::new("/fixture/data"));
        assert!(message.contains("afpAccessDenied"));
        assert!(message.contains("OSStatus -5000"));
        assert!(message.contains("does not identify the exact cause"));
        assert!(message.contains("not a guaranteed fix"));
        assert!(message.contains("No privilege escalation"));
        assert!(trash_failure(-43, Path::new("/fixture/data")).contains("OSStatus -43"));
    }
    #[test]
    fn restore_renames_a_recorded_object_and_never_overwrites_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let trash = root.join("trash");
        let original = root.join("original");
        std::fs::create_dir(&trash).unwrap();
        std::fs::create_dir(&original).unwrap();
        let source = trash.join("App 12-00.app");
        let destination = original.join("App.app");
        std::fs::write(&source, "approved").unwrap();
        let pin = Pinned::capture(&source).unwrap();
        let reference = Reference::capture(&source, &pin).unwrap();
        let parent = Pinned::capture(&original).unwrap();
        let parent_ref = Reference::capture(&original, &parent).unwrap();
        std::fs::write(&destination, "existing").unwrap();
        assert!(reference
            .restore_to(&pin, &parent_ref, &parent, &destination)
            .is_err());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "existing");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "approved");
        std::fs::remove_file(&destination).unwrap();
        let receipt = reference
            .restore_to(&pin, &parent_ref, &parent, &destination)
            .unwrap();
        assert_eq!(receipt.status, super::super::trash::MoveStatus::Moved);
        assert!(!source.exists());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "approved");
    }
}
