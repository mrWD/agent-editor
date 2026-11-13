//! What the app is told at start, and the one thing it remembers between starts.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The file in the data folder that holds the port the core listened on last time.
const PORT_FILE: &str = "shell-port";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// `--data-dir` or `AE_DATA_DIR`: where projects, tasks and working copies are kept.
    /// `None` means the same folder `ae-core` uses when it is told nothing.
    pub data_dir: Option<PathBuf>,
    /// `--ui-dir` or `AE_UI_DIR`: a folder with a built interface to show instead of
    /// the one the app carries inside. For working on the interface.
    pub ui_dir: Option<PathBuf>,
}

impl Settings {
    /// Reads the command line and the environment of this process.
    pub fn from_process() -> Self {
        Self::read(std::env::args().skip(1), |name| std::env::var_os(name))
    }

    /// A flag wins over its environment variable. Words the app does not know are
    /// passed over: a launcher may add its own, and the app must still open.
    pub fn read(args: impl IntoIterator<Item = String>, env: impl Fn(&str) -> Option<OsString>) -> Self {
        let mut data_dir = None;
        let mut ui_dir = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let (name, value) = match arg.split_once('=') {
                Some((name, value)) => (name.to_string(), Some(value.to_string())),
                None => (arg, None),
            };
            let slot = match name.as_str() {
                "--data-dir" => &mut data_dir,
                "--ui-dir" => &mut ui_dir,
                _ => continue,
            };
            // `--flag value` and `--flag=value` both work.
            *slot = value.or_else(|| args.next()).filter(|value| !value.is_empty()).map(PathBuf::from);
        }
        let from_env = |name: &str| env(name).filter(|value| !value.is_empty()).map(PathBuf::from);
        Self {
            data_dir: data_dir.or_else(|| from_env("AE_DATA_DIR")),
            ui_dir: ui_dir.or_else(|| from_env("AE_UI_DIR")),
        }
    }
}

/// The port the core had last time with this data folder, if the app noted one.
///
/// The interface keeps small things, such as which files were open, in the
/// window's own storage, and that storage is tied to the address. A new port
/// on every start would lose them every time.
pub fn remembered_port(data_dir: &Path) -> Option<u16> {
    let noted = std::fs::read_to_string(data_dir.join(PORT_FILE)).ok()?;
    noted.trim().parse::<u16>().ok().filter(|port| *port != 0)
}

/// Notes the port for the next start. Failing to is not worth stopping for:
/// the next start then simply takes any free port.
pub fn remember_port(data_dir: &Path, port: u16) {
    if remembered_port(data_dir) == Some(port) {
        return;
    }
    if let Err(error) = std::fs::write(data_dir.join(PORT_FILE), format!("{port}\n")) {
        tracing::warn!("Could not note the port for next time: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| word.to_string()).collect()
    }

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn with_nothing_said_the_defaults_are_used() {
        assert_eq!(Settings::read(words(&[]), no_env), Settings::default());
    }

    #[test]
    fn folders_are_read_from_flags_in_both_spellings() {
        let read = Settings::read(words(&["--data-dir", "/tmp/data", "--ui-dir=/tmp/ui"]), no_env);
        assert_eq!(read.data_dir, Some(PathBuf::from("/tmp/data")));
        assert_eq!(read.ui_dir, Some(PathBuf::from("/tmp/ui")));
    }

    #[test]
    fn folders_are_read_from_the_environment_and_a_flag_wins() {
        let env = |name: &str| match name {
            "AE_DATA_DIR" => Some(OsString::from("/env/data")),
            "AE_UI_DIR" => Some(OsString::from("/env/ui")),
            _ => None,
        };
        let read = Settings::read(words(&["--data-dir", "/flag/data"]), env);
        assert_eq!(read.data_dir, Some(PathBuf::from("/flag/data")));
        assert_eq!(read.ui_dir, Some(PathBuf::from("/env/ui")));
    }

    #[test]
    fn words_the_app_does_not_know_and_empty_values_are_passed_over() {
        let env = |name: &str| (name == "AE_DATA_DIR").then(OsString::new);
        let read = Settings::read(words(&["-psn_0_12345", "--unknown", "x", "file.txt", "--ui-dir="]), env);
        assert_eq!(read, Settings::default());
        // A flag at the very end with no value after it is passed over too.
        assert_eq!(Settings::read(words(&["--data-dir"]), no_env), Settings::default());
    }

    #[test]
    fn the_port_is_remembered_per_data_folder() {
        let dir = std::env::temp_dir().join(format!("ae-shell-port-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The folder is not there yet, as on the very first start.
        assert_eq!(remembered_port(&dir), None);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(remembered_port(&dir), None);

        remember_port(&dir, 51234);
        assert_eq!(remembered_port(&dir), Some(51234));
        remember_port(&dir, 51999);
        assert_eq!(remembered_port(&dir), Some(51999));

        // A file somebody damaged is as good as no file.
        for damaged in ["", "0", "seventy", "70000", "-1"] {
            std::fs::write(dir.join(PORT_FILE), damaged).unwrap();
            assert_eq!(remembered_port(&dir), None, "{damaged:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
