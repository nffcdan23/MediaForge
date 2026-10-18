//! File-system work for jobs: private working folders, destination checks, and moving finished files into
//! place without ever overwriting anything.

use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use uuid::Uuid;

use crate::error::MediaForgeError;
use crate::support::filename;

/// Refuse to start work when the destination has less than this available.
pub const MINIMUM_FREE_BYTES: u64 = 256 * 1024 * 1024;
/// How long a folder that isn't named for a process must sit untouched before it counts as abandoned.
pub const ABANDONED_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Parent of every running copy's working folder.
pub fn work_container() -> PathBuf {
    std::env::temp_dir().join("MediaForge").join("Jobs")
}

/// Where this process keeps in-progress downloads and conversions until they're complete.
///
/// Named for the process, so two copies of the app running at once never share, or clean up, each other's work.
pub fn default_work_root() -> PathBuf {
    work_container().join(std::process::id().to_string())
}

pub fn make_work_directory(root: &Path, job: Uuid, attempt: u32) -> Result<PathBuf, MediaForgeError> {
    let directory = root.join(format!("{}-{attempt}", job.as_hyphenated().to_string().to_uppercase()));
    let _ = std::fs::remove_dir_all(&directory);
    let create = || -> std::io::Result<()> {
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            if let Some(parent) = directory.parent() {
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    };
    create().map_err(|_| MediaForgeError::DestinationUnavailable("MediaForge couldn't create a temporary working folder.".into()))?;
    Ok(directory)
}

pub fn remove_work_directory(directory: &Path) {
    let _ = std::fs::remove_dir_all(directory);
}

/// Clears working folders left behind by copies of the app that are no longer running.
///
/// A folder named for a process that is still alive belongs to another running copy and is left alone.
/// Anything else is only removed once it has gone a day without changing.
pub fn purge_abandoned_work(container: &Path, own_process: u32, now: SystemTime, is_running: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(container) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Ok(owner) = name.parse::<u32>() {
            // Our own identifier can only be left over from an earlier process that had the same one.
            if owner != own_process && is_running(owner) {
                continue;
            }
        } else {
            let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(now);
            if now.duration_since(modified).unwrap_or_default() < ABANDONED_AFTER {
                continue;
            }
        }
        let path = entry.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

pub fn process_is_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid == 0 || pid > i32::MAX as u32 {
            return false;
        }
        let result = unsafe { libc::kill(pid as i32, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
            CloseHandle(handle);
            alive
        }
    }
}

fn folder_name(directory: &Path) -> String {
    directory.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| directory.display().to_string())
}

/// Makes sure the destination exists, is writable, and has room.
pub fn prepare_destination(directory: &Path) -> Result<(), MediaForgeError> {
    let name = folder_name(directory);
    if directory.exists() {
        if !directory.is_dir() {
            return Err(MediaForgeError::DestinationUnavailable(format!(
                "“{name}” is a file, not a folder. Choose a different destination in Settings."
            )));
        }
    } else if std::fs::create_dir_all(directory).is_err() {
        return Err(MediaForgeError::DestinationUnavailable(format!(
            "The folder “{name}” couldn't be created. Choose a different destination in Settings."
        )));
    }
    // Asking the system whether a folder is writable is unreliable across platforms; trying is not.
    let probe = directory.join(format!(".mediaforge-write-test-{}", Uuid::new_v4()));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(_) => {
            return Err(MediaForgeError::DestinationUnavailable(format!(
                "MediaForge doesn't have permission to save to “{name}”. Choose a different destination in Settings."
            )));
        }
    }
    if available_space(directory).is_some_and(|free| free < MINIMUM_FREE_BYTES) {
        return Err(MediaForgeError::InsufficientDiskSpace);
    }
    Ok(())
}

pub fn available_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
            return None;
        }
        Some(stats.f_bavail as u64 * stats.f_frsize as u64)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut available = 0u64;
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(available)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub path: PathBuf,
    pub size: u64,
}

fn is_out_of_space(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::StorageFull || error.raw_os_error() == Some(if cfg!(windows) { 112 } else { 28 })
}

/// Moves a finished file into the destination under a sanitised, non-colliding name.
///
/// The name is claimed first by creating it exclusively, so a file that appeared since the name was chosen is
/// never replaced. A move across disks (for example to an external drive) falls back to copying.
pub fn move_into_place(file: &Path, directory: &Path, base_name: &str, extension: &str) -> Result<Placed, MediaForgeError> {
    let name = folder_name(directory);
    let mut last_error: Option<std::io::Error> = None;
    for _ in 0..5 {
        let target = filename::unique_path(directory, base_name, extension);
        match OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(placeholder) => drop(placeholder),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                last_error = Some(error);
                continue;
            }
            Err(error) if is_out_of_space(&error) => return Err(MediaForgeError::InsufficientDiskSpace),
            Err(error) => {
                last_error = Some(error);
                break;
            }
        }
        // The name is ours now; replace our empty placeholder with the real file.
        if std::fs::rename(file, &target).is_err() {
            let copied = std::fs::File::open(file).and_then(|mut source| {
                let mut destination = OpenOptions::new().write(true).truncate(true).open(&target)?;
                std::io::copy(&mut source, &mut destination)?;
                destination.sync_all()
            });
            if let Err(error) = copied {
                let _ = std::fs::remove_file(&target);
                if is_out_of_space(&error) {
                    return Err(MediaForgeError::InsufficientDiskSpace);
                }
                return Err(MediaForgeError::DestinationUnavailable(format!("The finished file couldn't be saved to “{name}”. {error}")));
            }
            let _ = std::fs::remove_file(file);
        }
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        return Ok(Placed { path: target, size });
    }
    Err(MediaForgeError::DestinationUnavailable(format!(
        "The finished file couldn't be saved to “{name}”. {}",
        last_error.map(|e| e.to_string()).unwrap_or_default()
    )))
}

/// Marks a downloaded file the way a web browser would, so the operating system treats it with the same care:
/// the quarantine attribute on macOS, the Mark of the Web on Windows.
pub fn mark_as_downloaded(path: &Path, source_url: &str) {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let seconds = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let value = format!("0081;{seconds:x};MediaForge;");
        let _ = source_url;
        if let (Ok(c_path), Ok(c_name)) =
            (std::ffi::CString::new(path.as_os_str().as_bytes()), std::ffi::CString::new("com.apple.quarantine"))
        {
            unsafe {
                libc::setxattr(c_path.as_ptr(), c_name.as_ptr(), value.as_ptr() as *const libc::c_void, value.len(), 0, 0);
            }
        }
    }
    #[cfg(windows)]
    {
        let mut stream = path.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        let host =
            url::Url::parse(source_url).ok().map(|u| format!("{}://{}/", u.scheme(), u.host_str().unwrap_or(""))).unwrap_or_default();
        let content = format!("[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl={host}\r\n");
        let _ = std::fs::write(PathBuf::from(stream), content);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (path, source_url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_running_copy_works_in_a_folder_of_its_own() {
        let root = default_work_root();
        assert!(root.ends_with(std::process::id().to_string()));
        let dir = tempfile::tempdir().unwrap();
        let job = Uuid::new_v4();
        let first = make_work_directory(dir.path(), job, 1).unwrap();
        let second = make_work_directory(dir.path(), job, 2).unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir() && second.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn start_up_clean_up_removes_abandoned_work_but_never_another_running_copys() {
        let container = tempfile::tempdir().unwrap();
        for name in ["100", "200", "300", "stray-old", "stray-new"] {
            std::fs::create_dir_all(container.path().join(name).join("job")).unwrap();
        }
        let old = SystemTime::now() + ABANDONED_AFTER + Duration::from_secs(60);
        // "stray-new" is fresh relative to `now`; pretend "stray-old" is a day old by moving `now` forward only for it.
        purge_abandoned_work(container.path(), 300, SystemTime::now(), |pid| pid == 100 || pid == 300);
        assert!(container.path().join("100").exists(), "another running copy's work was removed");
        assert!(!container.path().join("200").exists(), "a dead copy's work was kept");
        assert!(!container.path().join("300").exists(), "a stale folder with our own number was kept");
        assert!(container.path().join("stray-new").exists());
        purge_abandoned_work(container.path(), 300, old, |pid| pid == 100);
        assert!(!container.path().join("stray-old").exists());
        // A container that doesn't exist yet is fine.
        purge_abandoned_work(&container.path().join("missing"), 1, SystemTime::now(), |_| false);
    }

    #[test]
    fn our_own_process_is_running() {
        assert!(process_is_running(std::process::id()));
        assert!(!process_is_running(0));
    }

    #[test]
    fn prepares_a_destination_and_reports_problems() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        prepare_destination(&nested).unwrap();
        assert!(nested.is_dir());
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(prepare_destination(&file), Err(MediaForgeError::DestinationUnavailable(_))));
        assert!(available_space(dir.path()).is_some_and(|free| free > 0));
    }

    #[test]
    fn never_overwrites_repeated_moves_get_numbered_names() {
        let work = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let mut placed = Vec::new();
        for index in 0..3 {
            let file = work.path().join(format!("converted-{index}.mp3"));
            std::fs::write(&file, format!("take {index}")).unwrap();
            placed.push(move_into_place(&file, destination.path(), "Theme", "mp3").unwrap());
            assert!(!file.exists());
        }
        let names: Vec<String> = placed.iter().map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Theme.mp3", "Theme 2.mp3", "Theme 3.mp3"]);
        assert_eq!(std::fs::read_to_string(&placed[0].path).unwrap(), "take 0");
        assert_eq!(placed[2].size, 6);
    }

    #[test]
    fn a_hostile_title_lands_inside_the_destination() {
        let work = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let file = work.path().join("converted.mp3");
        std::fs::write(&file, "x").unwrap();
        let placed = move_into_place(&file, destination.path(), "../../../etc/passwd", "mp3").unwrap();
        assert_eq!(placed.path.parent().unwrap(), destination.path());
    }
}
