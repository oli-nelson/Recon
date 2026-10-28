use ratatui::style::Color;
use std::time::{Duration, Instant};

pub const ACTIVITY: Duration = Duration::from_millis(800);

/// Schedule from actual presentation times, skipping missed frames instead of
/// accumulating animation work when a terminal or repository is slow.
pub struct FramePacer {
    interval: Duration,
    last: Instant,
}
impl FramePacer {
    pub fn new(fps: u16, now: Instant) -> Self {
        let interval = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
        Self {
            interval,
            last: now - interval,
        }
    }
    pub fn ready(&self, now: Instant) -> bool {
        now.duration_since(self.last) >= self.interval
    }
    pub fn presented(&mut self, now: Instant) {
        self.last = now;
    }
    pub fn wait(&self, now: Instant, pending: bool) -> Duration {
        let idle = Duration::from_millis(40);
        if pending {
            self.interval
                .saturating_sub(now.duration_since(self.last))
                .min(idle)
        } else {
            idle
        }
    }
}
pub fn progress(age: Duration, duration: Duration) -> f64 {
    (age.as_secs_f64() / duration.as_secs_f64().max(0.001)).clamp(0.0, 1.0)
}
/// A damped spring enters from the clipping edge, overshoots by at most two
/// cells, and settles without ever displacing the fixed Git status column.
pub fn slide(
    age: Duration,
    duration: Duration,
    enabled: bool,
    departing: bool,
    distance: u16,
) -> i16 {
    if !enabled || age >= duration {
        return 0;
    }
    let t = progress(age, duration);
    let offset = if departing {
        // Separate early so both the departing copy and the tombstone are visible.
        1.0 - (1.0 - t).powi(3)
    } else {
        (-5.5 * t).exp() * ((6.8 * t).cos() + 0.35 * (6.8 * t).sin())
    };
    (offset * distance as f64)
        .round()
        .clamp(-2.0, distance as f64) as i16
}
pub fn glow(color: Color, age: Duration, duration: Duration, enabled: bool, light: bool) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    if !enabled || age >= duration {
        return color;
    }
    let amount = (1.0 - progress(age, duration)).powi(2) * 0.65;
    let target = if light { 25.0 } else { 255.0 };
    let channel = |v: u8| (v as f64 + (target - v as f64) * amount).round() as u8;
    Color::Rgb(channel(r), channel(g), channel(b))
}

/// Rank modification times across affected paths, grouping equal timestamps.
/// The scale depends only on observed files, never on the wall clock.
pub fn relative_age(time: Option<std::time::SystemTime>, times: &[std::time::SystemTime]) -> f64 {
    let Some(time) = time else { return 1.0 };
    let Ok(index) = times.binary_search(&time) else {
        return 1.0;
    };
    if times.len() <= 1 {
        0.0
    } else {
        1.0 - index as f64 / (times.len() - 1) as f64
    }
}
pub fn gradient(age: f64, colors: [Color; 4]) -> Color {
    let position = age.clamp(0.0, 1.0) * 3.0;
    let index = (position.floor() as usize).min(2);
    let amount = position - index as f64;
    match (colors[index], colors[index + 1]) {
        (Color::Rgb(r, g, b), Color::Rgb(x, y, z)) => {
            let blend = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * amount).round() as u8;
            Color::Rgb(blend(r, x), blend(g, y), blend(b, z))
        }
        _ => colors[3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pacing_caps_active_frames_and_does_not_catch_up_after_stalls() {
        for fps in [30, 60, 120, 144, 240] {
            let start = Instant::now();
            let mut pacer = FramePacer::new(fps, start);
            assert!(pacer.ready(start));
            pacer.presented(start);
            let wait = pacer.wait(start, true);
            assert!(!pacer.ready(start + wait / 2));
            assert!(pacer.ready(start + Duration::from_secs_f64(1.0 / fps as f64)));
            let late = start + Duration::from_secs(1);
            pacer.presented(late);
            assert!(!pacer.ready(late));
            assert_eq!(pacer.wait(late, false), Duration::from_millis(40));
        }
    }
    #[test]
    fn motion_settles_and_accessibility_disables_displacement() {
        assert_eq!(slide(Duration::ZERO, ACTIVITY, true, false, 30), 30);
        assert_eq!(slide(ACTIVITY, ACTIVITY, true, false, 30), 0);
        assert!(slide(ACTIVITY / 2, ACTIVITY, true, false, 30) < 0);
        assert_eq!(slide(ACTIVITY / 2, ACTIVITY, false, false, 30), 0);
        let color = Color::Rgb(100, 180, 230);
        assert_eq!(glow(color, ACTIVITY, ACTIVITY, true, false), color);
        assert_eq!(glow(color, Duration::ZERO, ACTIVITY, false, false), color);
    }
}
