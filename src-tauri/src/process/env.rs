//! The minimal environment helper tools run in.
//!
//! Nothing is inherited wholesale, so stray `DYLD_*`, `LD_PRELOAD`, `PYTHONPATH` or shell configuration in the
//! parent can't change what the tools do.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Network configuration an organisation may impose. Passing it on keeps helpers on the sanctioned route.
const PASSTHROUGH: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
];

#[cfg(unix)]
const SYSTEM_KEYS: &[&str] = &["USER", "LOGNAME"];

/// What Windows programs (Python's sockets and SSL among them) need to start at all.
#[cfg(windows)]
const SYSTEM_KEYS: &[&str] = &[
    "SystemRoot",
    "windir",
    "SystemDrive",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "CommonProgramFiles",
    "PATHEXT",
    "COMSPEC",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    "USERNAME",
    "USERDOMAIN",
];

/// Directories that are always searched after the tools' own.
pub fn trusted_search_path() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"].iter().map(PathBuf::from).collect()
    }
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        vec![root.join("System32"), root.clone(), root.join("System32").join("Wbem")]
    }
}

/// Builds the environment for a helper, with `prepend` searched first.
pub fn sanitized(prepend: &[&Path]) -> Vec<(OsString, OsString)> {
    sanitized_from(prepend, |key| std::env::var_os(key))
}

pub fn sanitized_from(prepend: &[&Path], lookup: impl Fn(&str) -> Option<OsString>) -> Vec<(OsString, OsString)> {
    let mut path: Vec<PathBuf> = Vec::new();
    for directory in prepend.iter().map(|p| p.to_path_buf()).chain(trusted_search_path()) {
        if directory.is_absolute() && !path.contains(&directory) {
            path.push(directory);
        }
    }

    let mut environment: BTreeMap<String, OsString> = BTreeMap::new();
    environment.insert("PATH".into(), std::env::join_paths(&path).unwrap_or_default());
    environment.insert("PYTHONIOENCODING".into(), "utf-8".into());
    environment.insert("PYTHONUNBUFFERED".into(), "1".into());
    environment.insert("NO_COLOR".into(), "1".into());

    #[cfg(unix)]
    {
        let home = lookup("HOME").filter(|h| !h.is_empty()).unwrap_or_else(|| "/".into());
        environment.insert("HOME".into(), home);
        environment.insert("TMPDIR".into(), std::env::temp_dir().into_os_string());
        environment.insert("LANG".into(), "en_US.UTF-8".into());
        environment.insert("LC_ALL".into(), "en_US.UTF-8".into());
    }
    #[cfg(windows)]
    {
        // Python on Windows otherwise writes in the console's legacy code page.
        environment.insert("PYTHONUTF8".into(), "1".into());
    }

    for key in PASSTHROUGH.iter().chain(SYSTEM_KEYS) {
        if let Some(value) = lookup(key).filter(|v| !v.is_empty()) {
            environment.insert((*key).to_string(), value);
        }
    }
    environment.into_iter().map(|(k, v)| (OsString::from(k), v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(key: &str) -> Option<OsString> {
        match key {
            "HOME" => Some("/Users/test".into()),
            "HTTPS_PROXY" => Some("http://proxy.example:3128".into()),
            "DYLD_INSERT_LIBRARIES" | "PYTHONPATH" | "LD_PRELOAD" => Some("/evil".into()),
            "SystemRoot" => Some(r"C:\Windows".into()),
            _ => None,
        }
    }

    #[test]
    fn children_get_a_minimal_environment_not_the_apps() {
        let env = sanitized_from(&[], lookup);
        let keys: Vec<String> = env.iter().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        assert!(keys.contains(&"PATH".to_string()));
        assert!(keys.contains(&"HTTPS_PROXY".to_string()));
        for forbidden in ["DYLD_INSERT_LIBRARIES", "PYTHONPATH", "LD_PRELOAD"] {
            assert!(!keys.contains(&forbidden.to_string()), "{forbidden} leaked");
        }
    }

    #[test]
    fn the_search_path_only_contains_absolute_deduplicated_directories() {
        let tools = std::env::temp_dir().join("mediaforge-tools");
        let relative = Path::new("relative/dir");
        let env = sanitized_from(&[&tools, relative, &tools], lookup);
        let path = env.iter().find(|(k, _)| k == "PATH").unwrap().1.clone();
        let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(entries[0], tools);
        assert_eq!(entries.iter().filter(|e| **e == tools).count(), 1);
        assert!(entries.iter().all(|e| e.is_absolute()));
    }
}
