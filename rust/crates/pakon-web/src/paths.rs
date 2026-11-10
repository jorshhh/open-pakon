//! Where the app's files are: the root that holds `web/static`, `oem/`,
//! `profiles/`, `resources/`, `firmware/` (the repo, or the app bundle's
//! resources), and the folders the environment moves elsewhere.

use std::path::{Path, PathBuf};

/// The environment's value, when set and not empty.
pub fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// The user's home folder.
pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// `Path.expanduser()`: a leading `~` is the home folder.
pub fn expand_user(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home(),
        Some(rest) if rest.starts_with(['/', '\\']) => home().join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// `Path.resolve()`: absolute with links followed; a path that does not
/// exist is made absolute as it is.
pub fn resolve(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// `raw.with_name(raw.name + suffix)`: a file beside the raw.
pub fn beside(raw: &Path, suffix: &str) -> PathBuf {
    let mut s = raw.as_os_str().to_owned();
    s.push(suffix);
    s.into()
}

#[derive(Debug, Clone)]
pub struct Paths {
    /// `web/static`, `oem/`, `profiles/`, `backups/`.
    pub root: PathBuf,
    /// The captured scripts (`PAKON_RESOURCES`, else `<root>/resources`).
    pub resources: PathBuf,
    /// The OEM Intel HEX firmware (`PAKON_FIRMWARE`, else `<root>/firmware`).
    pub firmware: PathBuf,
    /// `settings.json` (`$PAKON_DATA`, else `<root>/backups`).
    pub settings: PathBuf,
}

fn is_root(dir: &Path) -> bool {
    dir.join("web").join("static").join("index.html").is_file()
}

/// The root: `PAKON_ROOT`; else the first folder above the executable or the
/// working directory that holds `web/static/index.html` (in a Mac bundle,
/// `Contents/Resources` beside `Contents/MacOS`).
pub fn find_root() -> Option<PathBuf> {
    if let Some(r) = env("PAKON_ROOT") {
        return Some(PathBuf::from(r));
    }
    let starts = [std::env::current_exe().ok().and_then(|p| dunce::canonicalize(p).ok()), std::env::current_dir().ok()];
    for start in starts.into_iter().flatten() {
        for dir in start.ancestors() {
            if is_root(dir) {
                return Some(dir.to_path_buf());
            }
            let res = dir.join("Resources");
            if is_root(&res) {
                return Some(res);
            }
        }
    }
    None
}

impl Paths {
    pub fn new(root: PathBuf) -> Paths {
        let dir = |key: &str, name: &str| env(key).map(PathBuf::from).unwrap_or_else(|| root.join(name));
        Paths {
            resources: dir("PAKON_RESOURCES", "resources"),
            firmware: dir("PAKON_FIRMWARE", "firmware"),
            settings: match env("PAKON_DATA") {
                Some(d) => expand_user(&d).join("settings.json"),
                None => root.join("backups").join("settings.json"),
            },
            root,
        }
    }

    pub fn static_dir(&self) -> PathBuf {
        self.root.join("web").join("static")
    }
    /// The captured firmware load: the same FX2 image for both models.
    pub fn pakfw(&self) -> PathBuf {
        self.resources.join("f135.pakfw")
    }
    pub fn fw_stage1(&self) -> PathBuf {
        self.firmware.join("PknLdr.hex")
    }
    pub fn fw_main(&self) -> PathBuf {
        self.firmware.join("Pakon7.hex")
    }
    pub fn fw_hex_available(&self) -> bool {
        self.fw_stage1().exists() && self.fw_main().exists()
    }
    pub fn pakscan_f135(&self) -> PathBuf {
        self.resources.join("36frames.pakscan")
    }
    pub fn pakscan_f135plus(&self) -> PathBuf {
        self.resources.join("f135plus").join("base16.pakscan")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_user_like_python() {
        assert_eq!(expand_user("~"), home());
        assert_eq!(expand_user("~/pakon"), home().join("pakon"));
        assert_eq!(expand_user("/tmp/~x"), PathBuf::from("/tmp/~x"));
        assert_eq!(expand_user("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn beside_appends_to_the_name() {
        assert_eq!(beside(Path::new("/a/scan.raw"), ".film"), PathBuf::from("/a/scan.raw.film"));
    }
}
