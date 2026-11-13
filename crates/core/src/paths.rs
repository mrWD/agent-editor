//! Paths as every program understands them.

use std::path::{Path, PathBuf};

/// The real path of a file or folder: links followed, `..` taken out.
///
/// On Windows the system's own answer starts with `\\?\`, a form that git, `cmd` and many other programs do not
/// take as a folder to work in. Where the plain form says the same, the plain form is given.
pub fn real_path(path: &Path) -> std::io::Result<PathBuf> {
    path.canonicalize().map(plain)
}

#[cfg(windows)]
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        // A share (`\\?\UNC\…`) and a path too long for the plain form stay as they are.
        Some(rest) if !rest.starts_with("UNC\\") && rest.len() < 248 => PathBuf::from(rest.to_string()),
        _ => path,
    }
}

#[cfg(not(windows))]
fn plain(path: PathBuf) -> PathBuf {
    path
}

/// For tests that name files of a made-up folder: the path as given, made absolute on this system too
/// ("/work/a" stays; on Windows it becomes "C:/work/a").
#[cfg(test)]
pub(crate) fn here(path: &str) -> String {
    if cfg!(windows) { format!("C:{path}") } else { path.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_path_is_one_other_programs_take() {
        let here = real_path(Path::new(".")).unwrap();
        assert!(here.is_absolute());
        assert!(!here.to_string_lossy().starts_with(r"\\?\"));
        assert!(real_path(Path::new("no/such/folder/anywhere")).is_err());
    }
}
