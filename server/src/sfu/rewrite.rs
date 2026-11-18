//! Keeps outgoing RTP time continuous when the source changes (simulcast layer
//! switch or a publisher that reconnected with new SSRCs and clock offsets).

use std::time::Instant;

#[derive(Debug, Clone)]
pub struct TimeRewriter {
    clock: u64,
    epoch: Instant,
    anchor: Option<(u64, u64)>,
    last_out: Option<u64>,
}

impl TimeRewriter {
    /// `clock` is the media clock rate (90 kHz video, 48 kHz Opus). Output time 0
    /// corresponds to `epoch`; anchors use arrival time so audio and video stay aligned.
    pub fn new(clock: u32, epoch: Instant) -> Self {
        TimeRewriter { clock: clock as u64, epoch, anchor: None, last_out: None }
    }

    /// The next mapped frame starts a new continuous run.
    pub fn reanchor(&mut self) {
        self.anchor = None;
    }

    pub fn map(&mut self, input: u64, arrival: Instant) -> u64 {
        let (in_base, out_base) = match self.anchor {
            Some(a) => a,
            None => {
                let wall = arrival.saturating_duration_since(self.epoch);
                let wall_ticks = (wall.as_micros() as u64).saturating_mul(self.clock) / 1_000_000;
                let out_base = match self.last_out {
                    Some(last) => wall_ticks.max(last + 1),
                    None => wall_ticks,
                };
                self.anchor = Some((input, out_base));
                (input, out_base)
            }
        };
        let out = if input >= in_base {
            out_base + (input - in_base)
        } else {
            out_base.saturating_sub(in_base - input)
        };
        // Never go backwards: decoders and muxers expect monotonic time.
        let out = match self.last_out {
            Some(last) if out < last => last,
            _ => out,
        };
        self.last_out = Some(out);
        out
    }
}

/// Per-subscriber video state: which simulcast layer is forwarded and which one
/// we are switching to (switches happen only on keyframes).
#[derive(Debug, Clone)]
pub struct VideoFwd {
    pub current: Option<usize>,
    pub target: Option<usize>,
    pub target_since: Option<Instant>,
    pub rw: TimeRewriter,
    /// Start on the first keyframe of any layer (viewers). The recorder waits for its target.
    fast_start: bool,
}

impl VideoFwd {
    pub fn new(epoch: Instant, fast_start: bool) -> Self {
        VideoFwd {
            current: None,
            target: None,
            target_since: None,
            rw: TimeRewriter::new(90_000, epoch),
            fast_start,
        }
    }

    /// Forget the source (new publisher): wait for a keyframe on the next target.
    pub fn reset(&mut self) {
        self.current = None;
        self.target = None;
        self.target_since = None;
        self.rw.reanchor();
    }

    /// Ask for `layer`. Returns true when a keyframe request is needed.
    pub fn want(&mut self, layer: usize, now: Instant) -> bool {
        if self.current == Some(layer) {
            self.target = None;
            self.target_since = None;
            return false;
        }
        if self.target != Some(layer) {
            self.target = Some(layer);
            self.target_since = Some(now);
            return true;
        }
        false
    }

    /// Decides whether a frame from `layer` is forwarded; returns its output time.
    ///
    /// Without a picture yet, the first keyframe of any layer starts playback (fast
    /// start); the pending target still switches in when its own keyframe arrives.
    pub fn accept(&mut self, layer: usize, keyframe: bool, input: u64, arrival: Instant) -> Option<u64> {
        let starting = self.fast_start && self.current.is_none();
        if keyframe && (self.target == Some(layer) || starting) {
            if self.current != Some(layer) {
                self.rw.reanchor();
            }
            self.current = Some(layer);
            if self.target == Some(layer) {
                self.target = None;
                self.target_since = None;
            }
        }
        if self.current != Some(layer) {
            return None;
        }
        Some(self.rw.map(input, arrival))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn continuous_within_source() {
        let t0 = Instant::now();
        let mut rw = TimeRewriter::new(90_000, t0);
        let a = rw.map(1_000_000, t0 + Duration::from_millis(100));
        let b = rw.map(1_003_000, t0 + Duration::from_millis(133));
        assert_eq!(a, 9_000); // 100 ms at 90 kHz
        assert_eq!(b - a, 3_000);
    }

    #[test]
    fn reanchor_follows_wall_clock_and_stays_monotonic() {
        let t0 = Instant::now();
        let mut rw = TimeRewriter::new(90_000, t0);
        rw.map(500, t0);
        let before = rw.map(500 + 90_000, t0 + Duration::from_secs(1));
        // New source with an unrelated clock offset, 2 s later.
        rw.reanchor();
        let after = rw.map(7_777_777, t0 + Duration::from_secs(3));
        assert_eq!(after, 270_000);
        assert!(after > before);
        // Reanchor that would land in the past is pushed forward.
        rw.reanchor();
        let next = rw.map(42, t0 + Duration::from_secs(2));
        assert_eq!(next, after + 1);
    }

    #[test]
    fn fast_start_on_any_keyframe_keeps_target() {
        let t0 = Instant::now();
        let mut v = VideoFwd::new(t0, true);
        v.want(2, t0);
        // The low layer's keyframe arrives first: start with it.
        assert!(v.accept(0, true, 100, t0).is_some());
        assert_eq!(v.current, Some(0));
        assert_eq!(v.target, Some(2), "still heading for the top layer");
        // Other layers' keyframes no longer hijack playback.
        assert!(v.accept(1, true, 100, t0).is_none());
        assert!(v.accept(2, true, 100, t0 + Duration::from_millis(40)).is_some());
        assert_eq!(v.current, Some(2));
        assert_eq!(v.target, None);
    }

    #[test]
    fn video_switches_only_on_keyframe() {
        let t0 = Instant::now();
        let mut v = VideoFwd::new(t0, false);
        assert!(v.want(1, t0));
        assert!(!v.want(1, t0), "no duplicate keyframe requests");
        // Delta frame of the target layer is not forwarded yet.
        assert_eq!(v.accept(1, false, 10, t0), None);
        assert!(v.accept(1, true, 20, t0).is_some());
        assert_eq!(v.current, Some(1));
        // Ask for the top layer; layer 1 keeps flowing until layer 2 sends a keyframe.
        assert!(v.want(2, t0));
        assert!(v.accept(1, false, 3020, t0 + Duration::from_millis(33)).is_some());
        assert_eq!(v.accept(2, false, 99, t0), None);
        let out = v.accept(2, true, 555, t0 + Duration::from_millis(66)).unwrap();
        assert_eq!(v.current, Some(2));
        assert!(v.accept(1, false, 6020, t0).is_none());
        assert!(out > 0);
    }
}
