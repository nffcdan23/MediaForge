//! Finds the helper tools: first a location the user chose, then the copies shipped inside the app, then a
//! fixed list of well-known install folders.
//!
//! `PATH` and the user's shell are never consulted, so a folder someone else can write to can't shadow a
//! tool. MediaForge only ever locates tools; it never downloads or installs them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::process::{self, ProcessRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tool {
    #[serde(rename = "yt-dlp")]
    YtDlp,
    #[serde(rename = "ffmpeg")]
    Ffmpeg,
    #[serde(rename = "ffprobe")]
    Ffprobe,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::YtDlp, Tool::Ffmpeg, Tool::Ffprobe];

    pub fn id(self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
        }
    }

    pub fn parse(text: &str) -> Option<Tool> {
        Self::ALL.into_iter().find(|t| t.id() == text)
    }

    pub fn executable_name(self) -> String {
        format!("{}{}", self.id(), std::env::consts::EXE_SUFFIX)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "FFmpeg",
            Self::Ffprobe => "FFprobe",
        }
    }

    pub fn purpose(self) -> &'static str {
        match self {
            Self::YtDlp => "Reads and downloads media from supported sources",
            Self::Ffmpeg => "Converts and processes audio and video",
            Self::Ffprobe => "Inspects media files (part of FFmpeg)",
        }
    }

    /// What to do when the tool can't be found. Packaged builds carry every tool, so a missing one means a
    /// damaged installation or a custom location that has gone away.
    pub fn missing_advice(self) -> String {
        let name = self.display_name();
        if cfg!(target_os = "macos") {
            format!(
                "MediaForge needs {name} to do this. Reinstall MediaForge, or choose a copy of {name} in Settings ▸ Tools (for example one installed with Homebrew: {}).",
                self.install_hint()
            )
        } else {
            format!("MediaForge needs {name} to do this. Reinstall MediaForge, or choose a copy of {name} in Settings ▸ Tools.")
        }
    }

    pub fn install_hint(self) -> &'static str {
        if cfg!(target_os = "macos") {
            match self {
                Self::YtDlp => "brew install yt-dlp",
                Self::Ffmpeg | Self::Ffprobe => "brew install ffmpeg",
            }
        } else if cfg!(windows) {
            match self {
                Self::YtDlp => "winget install yt-dlp.yt-dlp",
                Self::Ffmpeg | Self::Ffprobe => "winget install Gyan.FFmpeg",
            }
        } else {
            match self {
                Self::YtDlp => "pipx install yt-dlp",
                Self::Ffmpeg | Self::Ffprobe => "sudo apt install ffmpeg",
            }
        }
    }

    fn version_arguments(self) -> &'static [&'static str] {
        match self {
            Self::YtDlp => &["--version"],
            Self::Ffmpeg | Self::Ffprobe => &["-version"],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "path", rename_all = "camelCase")]
pub enum ToolLocationError {
    NotFound,
    NotAbsolute(String),
    NotExecutable(String),
    InsecurePermissions(String),
}

impl ToolLocationError {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound => "Not installed".into(),
            Self::NotAbsolute(path) => format!("“{path}” isn't an absolute path."),
            Self::NotExecutable(path) => format!("“{path}” isn't an executable file."),
            Self::InsecurePermissions(path) => {
                format!("“{path}” can be modified by any user, so MediaForge won't run it.")
            }
        }
    }
}

/// The helper tools a packaged copy of MediaForge carries inside itself.
///
/// - macOS: FFmpeg, FFprobe and Deno are Tauri sidecars next to the app's executable (`Contents/MacOS`);
///   yt-dlp's self-contained folder is a resource (`Contents/Resources/tools/yt-dlp`).
/// - Windows: yt-dlp's folder and FFmpeg's (programs plus their DLLs) are resources under `tools\` in the
///   install folder; Deno is a sidecar next to `MediaForge.exe`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BundledTools {
    pub locations: BTreeMap<Tool, PathBuf>,
    /// Folders of programs the tools start themselves (the JavaScript runtime yt-dlp needs for some sites).
    /// Put on the helpers' search path.
    pub support_directories: Vec<PathBuf>,
}

impl BundledTools {
    pub fn discover(executable_directory: Option<&Path>, resource_directory: Option<&Path>) -> Self {
        let mut result = BundledTools::default();
        let suffix = std::env::consts::EXE_SUFFIX;
        for tool in Tool::ALL {
            let mut candidates = Vec::new();
            if let Some(resources) = resource_directory {
                let folder = match tool {
                    Tool::YtDlp => "yt-dlp",
                    Tool::Ffmpeg | Tool::Ffprobe => "ffmpeg",
                };
                candidates.push(resources.join("tools").join(folder).join(tool.executable_name()));
            }
            if let Some(executables) = executable_directory {
                candidates.push(executables.join(tool.executable_name()));
            }
            if let Some(found) = candidates.into_iter().find(|c| validate_executable(c).is_ok()) {
                result.locations.insert(tool, found);
            }
        }
        let deno = format!("deno{suffix}");
        let support_candidates = [executable_directory.map(Path::to_path_buf), resource_directory.map(|r| r.join("tools").join("deno"))];
        for directory in support_candidates.into_iter().flatten() {
            if validate_executable(&directory.join(&deno)).is_ok() && !result.support_directories.contains(&directory) {
                result.support_directories.push(directory);
            }
        }
        result
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.locations.values().any(|p| p == path)
    }
}

/// Well-known install folders, searched after the bundled copies.
pub fn default_search_directories() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"].iter().map(PathBuf::from).collect()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        ["/usr/local/bin", "/usr/bin", "/snap/bin"].iter().map(PathBuf::from).collect()
    }
    #[cfg(windows)]
    {
        let mut directories = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            directories.push(PathBuf::from(local).join("Microsoft").join("WinGet").join("Links"));
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            directories.push(PathBuf::from(profile).join("scoop").join("shims"));
        }
        if let Some(data) = std::env::var_os("ProgramData") {
            directories.push(PathBuf::from(data).join("chocolatey").join("bin"));
        }
        directories
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolLocator {
    pub search_directories: Vec<PathBuf>,
    /// Explicit locations chosen in Settings. An override that fails validation is an error, not a fallback.
    pub overrides: BTreeMap<Tool, String>,
    pub bundled: BundledTools,
}

impl ToolLocator {
    pub fn locate(&self, tool: Tool) -> Result<PathBuf, ToolLocationError> {
        if let Some(path) = self.overrides.get(&tool).map(|p| p.trim()).filter(|p| !p.is_empty()) {
            return validate_executable(&expand_home(path));
        }
        if let Some(included) = self.bundled.locations.get(&tool)
            && let Ok(path) = validate_executable(included)
        {
            return Ok(path);
        }
        for directory in &self.search_directories {
            if let Ok(path) = validate_executable(&directory.join(tool.executable_name())) {
                return Ok(path);
            }
        }
        Err(ToolLocationError::NotFound)
    }
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
        && let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Accepts only an absolute path to a regular, executable file that other users can't modify.
pub fn validate_executable(path: &Path) -> Result<PathBuf, ToolLocationError> {
    let display = path.display().to_string();
    if !path.is_absolute() {
        return Err(ToolLocationError::NotAbsolute(display));
    }
    // Follows symlinks: Homebrew's stable link is kept as given, but the file it points at is what's checked.
    let metadata = std::fs::metadata(path).map_err(|_| ToolLocationError::NotExecutable(display.clone()))?;
    if !metadata.is_file() {
        return Err(ToolLocationError::NotExecutable(display));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o111 == 0 {
            return Err(ToolLocationError::NotExecutable(display));
        }
        if mode & 0o002 != 0 {
            return Err(ToolLocationError::InsecurePermissions(display));
        }
    }
    #[cfg(windows)]
    {
        let is_exe = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
        if !is_exe {
            return Err(ToolLocationError::NotExecutable(display));
        }
    }
    Ok(path.to_path_buf())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub tool: Tool,
    pub name: &'static str,
    pub purpose: &'static str,
    pub path: Option<String>,
    pub version: Option<String>,
    pub bundled: bool,
    pub custom: bool,
    pub error: Option<String>,
    pub install_hint: &'static str,
}

/// The set of tools a job can use, captured when the job starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toolchain {
    pub ytdlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub support_directories: Vec<PathBuf>,
}

/// Tracks which helper tools are available, for both the engine and the UI.
pub struct ToolManager {
    state: Mutex<ToolManagerState>,
    on_change: Box<dyn Fn() + Send + Sync>,
}

struct ToolManagerState {
    locator: ToolLocator,
    found: BTreeMap<Tool, Result<PathBuf, ToolLocationError>>,
    versions: BTreeMap<PathBuf, String>,
    generation: u64,
}

impl ToolManager {
    pub fn new(locator: ToolLocator, on_change: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        let found = Tool::ALL.into_iter().map(|t| (t, locator.locate(t))).collect();
        Arc::new(Self {
            state: Mutex::new(ToolManagerState { locator, found, versions: BTreeMap::new(), generation: 0 }),
            on_change: Box::new(on_change),
        })
    }

    pub fn toolchain(&self) -> Toolchain {
        let state = self.state.lock().unwrap();
        let get = |tool| state.found.get(&tool).and_then(|r| r.as_ref().ok()).cloned();
        Toolchain {
            ytdlp: get(Tool::YtDlp),
            ffmpeg: get(Tool::Ffmpeg),
            ffprobe: get(Tool::Ffprobe),
            support_directories: state.locator.bundled.support_directories.clone(),
        }
    }

    pub fn missing(&self) -> Vec<Tool> {
        let state = self.state.lock().unwrap();
        Tool::ALL.into_iter().filter(|t| !matches!(state.found.get(t), Some(Ok(_)))).collect()
    }

    pub fn can_convert(&self) -> bool {
        let missing = self.missing();
        !missing.contains(&Tool::Ffmpeg) && !missing.contains(&Tool::Ffprobe)
    }

    pub fn statuses(&self) -> Vec<ToolStatus> {
        let state = self.state.lock().unwrap();
        Tool::ALL
            .into_iter()
            .map(|tool| {
                let result = state.found.get(&tool).cloned().unwrap_or(Err(ToolLocationError::NotFound));
                let custom = state.locator.overrides.get(&tool).is_some_and(|p| !p.trim().is_empty());
                match result {
                    Ok(path) => ToolStatus {
                        tool,
                        name: tool.display_name(),
                        purpose: tool.purpose(),
                        version: state.versions.get(&path).cloned(),
                        bundled: state.locator.bundled.contains(&path),
                        custom,
                        path: Some(path.display().to_string()),
                        error: None,
                        install_hint: tool.install_hint(),
                    },
                    Err(error) => ToolStatus {
                        tool,
                        name: tool.display_name(),
                        purpose: tool.purpose(),
                        path: None,
                        version: None,
                        bundled: false,
                        custom,
                        error: Some(error.message()),
                        install_hint: tool.install_hint(),
                    },
                }
            })
            .collect()
    }

    pub fn path(&self, tool: Tool) -> Option<PathBuf> {
        self.state.lock().unwrap().found.get(&tool).and_then(|r| r.as_ref().ok()).cloned()
    }

    /// Re-scans for tools, then fills in version numbers in the background.
    pub fn refresh(self: &Arc<Self>, overrides: Option<BTreeMap<Tool, String>>) {
        let (paths, generation) = {
            let mut state = self.state.lock().unwrap();
            if let Some(overrides) = overrides {
                state.locator.overrides = overrides;
            }
            let locator = state.locator.clone();
            state.found = Tool::ALL.into_iter().map(|t| (t, locator.locate(t))).collect();
            state.generation += 1;
            let paths: Vec<(Tool, PathBuf)> = state
                .found
                .iter()
                .filter_map(|(tool, r)| r.as_ref().ok().map(|p| (*tool, p.clone())))
                .filter(|(_, p)| !state.versions.contains_key(p))
                .collect();
            (paths, state.generation)
        };
        (self.on_change)();
        if paths.is_empty() {
            return;
        }
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let lookups = paths.into_iter().map(|(tool, path)| async move {
                let version = version_of(tool, &path).await;
                (path, version)
            });
            let results = futures_join_all(lookups).await;
            {
                let mut state = manager.state.lock().unwrap();
                if state.generation != generation {
                    return;
                }
                for (path, version) in results {
                    if let Some(version) = version {
                        state.versions.insert(path, version);
                    }
                }
            }
            (manager.on_change)();
        });
    }
}

async fn futures_join_all<F: std::future::Future>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    let mut outputs = Vec::new();
    // Three tools at most: running them one after another keeps this dependency-free and is fast enough.
    for future in futures {
        outputs.push(future.await);
    }
    outputs
}

pub async fn version_of(tool: Tool, path: &Path) -> Option<String> {
    let mut request = ProcessRequest::new(path, tool.version_arguments().iter().map(Into::into).collect());
    request.timeout = Some(Duration::from_secs(20));
    request.max_output_bytes = 256 * 1024;
    request.working_directory = Some(std::env::temp_dir());
    let output = process::run_collect(&request, &CancellationToken::new()).await.ok()?;
    if !output.exit.success() {
        return None;
    }
    parse_version(&String::from_utf8_lossy(&output.stdout), tool)
}

/// yt-dlp prints a bare version; FFmpeg prints "ffmpeg version 7.1 Copyright …".
pub fn parse_version(text: &str, tool: Tool) -> Option<String> {
    let line = text.lines().next()?;
    let words: Vec<&str> = line.split_whitespace().collect();
    match tool {
        Tool::YtDlp => words.first().map(|w| w.to_string()),
        Tool::Ffmpeg | Tool::Ffprobe => {
            let index = words.iter().position(|w| *w == "version")?;
            words.get(index + 1).map(|w| w.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(directory: &Path, name: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, if cfg!(windows) { "MZ" } else { "#!/bin/sh\necho 1.0\n" }).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn finds_a_tool_in_a_search_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = make_tool(dir.path(), &Tool::Ffmpeg.executable_name());
        let locator = ToolLocator { search_directories: vec![dir.path().to_path_buf()], ..Default::default() };
        assert_eq!(locator.locate(Tool::Ffmpeg), Ok(path));
        assert_eq!(locator.locate(Tool::YtDlp), Err(ToolLocationError::NotFound));
    }

    #[test]
    fn refuses_relative_paths_non_executables_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(validate_executable(Path::new("ffmpeg")), Err(ToolLocationError::NotAbsolute(_))));
        assert!(matches!(validate_executable(dir.path()), Err(ToolLocationError::NotExecutable(_))));
        let plain = dir.path().join("notes.txt");
        std::fs::write(&plain, "x").unwrap();
        assert!(matches!(validate_executable(&plain), Err(ToolLocationError::NotExecutable(_))));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let open = make_tool(dir.path(), "open-tool");
            std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
            assert!(matches!(validate_executable(&open), Err(ToolLocationError::InsecurePermissions(_))));
        }
    }

    #[test]
    fn order_of_preference_is_override_then_bundled_then_installed() {
        let installed = tempfile::tempdir().unwrap();
        let bundle = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        let name = Tool::Ffmpeg.executable_name();
        let installed_path = make_tool(installed.path(), &name);
        let bundled_path = make_tool(bundle.path(), &name);
        let custom_path = make_tool(custom.path(), &name);

        let mut locator = ToolLocator { search_directories: vec![installed.path().to_path_buf()], ..Default::default() };
        assert_eq!(locator.locate(Tool::Ffmpeg), Ok(installed_path));
        locator.bundled = BundledTools::discover(Some(bundle.path()), None);
        assert_eq!(locator.locate(Tool::Ffmpeg), Ok(bundled_path.clone()));
        assert!(locator.bundled.contains(&bundled_path));
        locator.overrides.insert(Tool::Ffmpeg, custom_path.display().to_string());
        assert_eq!(locator.locate(Tool::Ffmpeg), Ok(custom_path));
        // A broken override is reported, never silently replaced.
        locator.overrides.insert(Tool::Ffmpeg, "/nonexistent/ffmpeg".into());
        assert!(locator.locate(Tool::Ffmpeg).is_err());
    }

    #[test]
    fn finds_each_tool_at_its_place_in_a_packaged_app() {
        let app = tempfile::tempdir().unwrap();
        let executables = app.path().join("MacOS");
        let resources = app.path().join("Resources");
        std::fs::create_dir_all(resources.join("tools").join("yt-dlp")).unwrap();
        std::fs::create_dir_all(resources.join("tools").join("ffmpeg")).unwrap();
        std::fs::create_dir_all(&executables).unwrap();
        let ytdlp = make_tool(&resources.join("tools").join("yt-dlp"), &Tool::YtDlp.executable_name());
        let ffmpeg = make_tool(&executables, &Tool::Ffmpeg.executable_name());
        let ffprobe = make_tool(&resources.join("tools").join("ffmpeg"), &Tool::Ffprobe.executable_name());
        make_tool(&executables, &format!("deno{}", std::env::consts::EXE_SUFFIX));

        let bundled = BundledTools::discover(Some(&executables), Some(&resources));
        assert_eq!(bundled.locations.get(&Tool::YtDlp), Some(&ytdlp));
        assert_eq!(bundled.locations.get(&Tool::Ffmpeg), Some(&ffmpeg));
        assert_eq!(bundled.locations.get(&Tool::Ffprobe), Some(&ffprobe));
        assert_eq!(bundled.support_directories, vec![executables]);

        let empty = tempfile::tempdir().unwrap();
        assert_eq!(BundledTools::discover(Some(empty.path()), Some(empty.path())), BundledTools::default());
    }

    #[test]
    fn parses_version_banners() {
        assert_eq!(parse_version("2026.08.19\n", Tool::YtDlp).as_deref(), Some("2026.08.19"));
        assert_eq!(
            parse_version("ffmpeg version 9.0.2 Copyright (c) 2000-2026 the FFmpeg developers\nbuilt with", Tool::Ffmpeg).as_deref(),
            Some("9.0.2")
        );
        assert_eq!(
            parse_version("ffprobe version n9.0.2-17-g2a571b6068 Copyright", Tool::Ffprobe).as_deref(),
            Some("n9.0.2-17-g2a571b6068")
        );
        assert_eq!(parse_version("", Tool::Ffmpeg), None);
    }
}
