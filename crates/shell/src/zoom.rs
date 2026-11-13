//! How large the interface is drawn in the window, and remembering it between starts.
//!
//! This is the zoom of a browser: the whole page grows or shrinks, text and
//! spacing together. In a browser the browser does it; in the app's window
//! nothing would, so the app has menu items for it with the keys everyone knows.

use std::path::Path;

/// The file in the data folder that holds the size the interface was left at.
const ZOOM_FILE: &str = "shell-zoom";

/// The sizes one can step through, the same ones browsers offer. 1.0 is the size as designed.
const LEVELS: [f64; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// The size as designed.
pub const ACTUAL: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    In,
    Out,
}

/// Sizes closer than this are one size: what was written to a file and read back is still found among the levels.
const SAME: f64 = 0.005;

/// The next size up or down from `current`. At either end it stays where it is.
pub fn step(current: f64, step: Step) -> f64 {
    let found = match step {
        Step::In => LEVELS.iter().copied().find(|level| *level > current + SAME),
        Step::Out => LEVELS.iter().rev().copied().find(|level| *level < current - SAME),
    };
    found.unwrap_or_else(|| fit(current))
}

/// Brings any number to a size the window can be drawn at.
fn fit(zoom: f64) -> f64 {
    if !zoom.is_finite() {
        return ACTUAL;
    }
    zoom.clamp(LEVELS[0], LEVELS[LEVELS.len() - 1])
}

/// The size the interface was left at with this data folder; the designed size when nothing was noted.
pub fn remembered(data_dir: &Path) -> f64 {
    std::fs::read_to_string(data_dir.join(ZOOM_FILE))
        .ok()
        .and_then(|noted| noted.trim().parse::<f64>().ok())
        .filter(|zoom| zoom.is_finite() && *zoom > 0.0)
        .map_or(ACTUAL, fit)
}

/// Notes the size for the next start. Failing to is not worth stopping for:
/// the next start then simply opens at the designed size.
pub fn remember(data_dir: &Path, zoom: f64) {
    if let Err(error) = std::fs::write(data_dir.join(ZOOM_FILE), format!("{zoom}\n")) {
        tracing::warn!("Could not note the size of the interface for next time: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_goes_through_the_levels_one_at_a_time() {
        assert_eq!(step(1.0, Step::In), 1.1);
        assert_eq!(step(1.1, Step::In), 1.25);
        assert_eq!(step(1.0, Step::Out), 0.9);
        assert_eq!(step(0.9, Step::Out), 0.8);
        // There and back again ends where it began.
        assert_eq!(step(step(1.0, Step::In), Step::Out), 1.0);
    }

    #[test]
    fn stepping_stops_at_both_ends() {
        assert_eq!(step(2.0, Step::In), 2.0);
        assert_eq!(step(0.5, Step::Out), 0.5);
    }

    #[test]
    fn a_size_between_levels_steps_to_the_nearest_one_on_that_side() {
        assert_eq!(step(1.2, Step::In), 1.25);
        assert_eq!(step(1.2, Step::Out), 1.1);
        // Anything outside comes back inside.
        assert_eq!(step(7.0, Step::In), 2.0);
        assert_eq!(step(7.0, Step::Out), 2.0);
        assert_eq!(step(f64::NAN, Step::In), ACTUAL);
    }

    #[test]
    fn the_size_is_remembered_per_data_folder() {
        let dir = std::env::temp_dir().join(format!("ae-shell-zoom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The folder is not there yet, as on the very first start.
        assert_eq!(remembered(&dir), ACTUAL);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(remembered(&dir), ACTUAL);

        remember(&dir, 1.25);
        assert_eq!(remembered(&dir), 1.25);
        // What was read back steps on as if it had never been written down.
        assert_eq!(step(remembered(&dir), Step::In), 1.5);

        // A file somebody damaged is as good as no file; a size out of range comes back inside.
        for damaged in ["", "0", "big", "-1", "NaN", "inf"] {
            std::fs::write(dir.join(ZOOM_FILE), damaged).unwrap();
            assert_eq!(remembered(&dir), ACTUAL, "{damaged:?}");
        }
        std::fs::write(dir.join(ZOOM_FILE), "40").unwrap();
        assert_eq!(remembered(&dir), 2.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
