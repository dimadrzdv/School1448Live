//! Simulcast layer bookkeeping and the per-viewer allocator.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Index 0 = low ("q"), 1 = mid ("h"), 2 = high ("f" or a single, non-simulcast layer).
pub const LAYERS: usize = 3;
const ACTIVE_FOR: Duration = Duration::from_millis(1500);
const WINDOW: Duration = Duration::from_millis(500);

pub fn layer_of_rid(rid: Option<&str>) -> usize {
    match rid {
        None => 2,
        Some("q" | "l" | "low" | "0") => 0,
        Some("h" | "m" | "mid" | "1") => 1,
        Some(_) => 2,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerPref {
    Auto,
    Low,
    Mid,
    High,
}

impl LayerPref {
    fn fixed(self) -> Option<usize> {
        match self {
            LayerPref::Auto => None,
            LayerPref::Low => Some(0),
            LayerPref::Mid => Some(1),
            LayerPref::High => Some(2),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LayerStat {
    pub bitrate: u64,
    window_bytes: u64,
    window_start: Option<Instant>,
    pub last_seen: Option<Instant>,
    pub last_keyframe: Option<Instant>,
}

#[derive(Debug, Clone, Default)]
pub struct LayerSet {
    pub layers: [LayerStat; LAYERS],
}

impl LayerSet {
    pub fn reset(&mut self) {
        *self = LayerSet::default();
    }

    pub fn on_frame(&mut self, layer: usize, bytes: usize, keyframe: bool, now: Instant) {
        let l = &mut self.layers[layer];
        l.last_seen = Some(now);
        if keyframe {
            l.last_keyframe = Some(now);
        }
        let start = *l.window_start.get_or_insert(now);
        l.window_bytes += bytes as u64;
        let elapsed = now.duration_since(start);
        if elapsed >= WINDOW {
            let measured = (l.window_bytes * 8) as f64 / elapsed.as_secs_f64();
            // Smooth: keyframes make single windows spiky.
            l.bitrate = if l.bitrate == 0 {
                measured as u64
            } else {
                (l.bitrate as f64 * 0.7 + measured * 0.3) as u64
            };
            l.window_bytes = 0;
            l.window_start = Some(now);
        }
    }

    pub fn is_active(&self, layer: usize, now: Instant) -> bool {
        self.layers[layer].last_seen.is_some_and(|t| now.duration_since(t) < ACTIVE_FOR)
    }

    /// Active layers, low to high, with a usable bitrate figure.
    pub fn active(&self, now: Instant) -> Vec<(usize, u64)> {
        (0..LAYERS)
            .filter(|l| self.is_active(*l, now))
            .map(|l| {
                let fallback = [300_000, 1_000_000, 3_000_000][l];
                let b = self.layers[l].bitrate;
                (l, if b > 0 { b } else { fallback })
            })
            .collect()
    }

    pub fn highest_active(&self, now: Instant) -> Option<usize> {
        (0..LAYERS).rev().find(|l| self.is_active(*l, now))
    }
}

#[derive(Debug, Clone)]
pub struct Demand {
    pub pref: LayerPref,
    pub bwe_bps: u64,
    /// Viewer is on the local network: does not use the internet uplink.
    pub lan: bool,
    pub current: Option<usize>,
    /// Consecutive allocation rounds in which a higher layer would fit.
    pub upgrade_streak: u8,
    /// Just joined: go straight to the best layer (no step-by-step climb).
    pub fresh: bool,
}

const UPGRADE_ROUNDS: u8 = 2;
const HEADROOM: f64 = 0.85;

/// Chooses a layer for every viewer. `active` is sorted low to high.
/// Returns the chosen layer per viewer (same order) and updates upgrade streaks.
pub fn allocate(active: &[(usize, u64)], viewers: &mut [Demand], budget_bps: u64) -> Vec<Option<usize>> {
    if active.is_empty() {
        for v in viewers.iter_mut() {
            v.upgrade_streak = 0;
        }
        return vec![None; viewers.len()];
    }
    let rate = |layer: usize| active.iter().find(|(l, _)| *l == layer).map(|(_, b)| *b).unwrap_or(0);
    let lowest = active[0].0;

    let mut chosen: Vec<usize> = viewers
        .iter_mut()
        .map(|v| {
            let fits = |layer: usize| (rate(layer) as f64) <= v.bwe_bps as f64 * HEADROOM;
            match v.pref.fixed() {
                Some(want) => active
                    .iter()
                    .rev()
                    .map(|(l, _)| *l)
                    .find(|l| *l <= want)
                    .unwrap_or(lowest),
                None => {
                    let by_bwe = active.iter().rev().map(|(l, _)| *l).find(|l| fits(*l)).unwrap_or(lowest);
                    match v.current.filter(|c| active.iter().any(|(l, _)| l == c)) {
                        Some(cur) if by_bwe > cur && !v.fresh => {
                            v.upgrade_streak = v.upgrade_streak.saturating_add(1);
                            if v.upgrade_streak >= UPGRADE_ROUNDS {
                                v.upgrade_streak = 0;
                                // Climb one step at a time.
                                active.iter().map(|(l, _)| *l).find(|l| *l > cur).unwrap_or(cur)
                            } else {
                                cur
                            }
                        }
                        _ => {
                            v.upgrade_streak = 0;
                            by_bwe
                        }
                    }
                }
            }
        })
        .collect();

    // Shared uplink: step the heaviest internet viewers down until the sum fits.
    let mut total: u64 = viewers
        .iter()
        .zip(&chosen)
        .filter(|(v, _)| !v.lan)
        .map(|(_, l)| rate(*l))
        .sum();
    while total > budget_bps {
        let candidate = chosen
            .iter()
            .enumerate()
            .filter(|(i, l)| !viewers[*i].lan && **l > lowest)
            .max_by_key(|(i, l)| (**l, *i))
            .map(|(i, _)| i);
        let Some(i) = candidate else { break };
        let from = chosen[i];
        let to = active.iter().rev().map(|(l, _)| *l).find(|l| *l < from).unwrap_or(lowest);
        total = total - rate(from) + rate(to);
        chosen[i] = to;
        viewers[i].upgrade_streak = 0;
    }

    chosen.into_iter().map(Some).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE: [(usize, u64); 3] = [(0, 300_000), (1, 1_200_000), (2, 4_500_000)];

    fn viewer(bwe: u64) -> Demand {
        Demand { pref: LayerPref::Auto, bwe_bps: bwe, lan: false, current: None, upgrade_streak: 0, fresh: false }
    }

    #[test]
    fn new_viewer_gets_what_fits() {
        let mut v = [viewer(1_500_000), viewer(10_000_000), viewer(100_000)];
        let r = allocate(&ACTIVE, &mut v, u64::MAX);
        assert_eq!(r, vec![Some(1), Some(2), Some(0)]);
    }

    #[test]
    fn upgrades_need_two_rounds_downgrades_are_immediate() {
        let mut v = [Demand { current: Some(1), ..viewer(10_000_000) }];
        assert_eq!(allocate(&ACTIVE, &mut v, u64::MAX), vec![Some(1)]);
        assert_eq!(allocate(&ACTIVE, &mut v, u64::MAX), vec![Some(2)]);
        let mut v = [Demand { current: Some(2), ..viewer(1_000_000) }];
        assert_eq!(allocate(&ACTIVE, &mut v, u64::MAX), vec![Some(0)]);
    }

    #[test]
    fn fresh_viewer_jumps_to_best_layer() {
        let mut v = [Demand { current: Some(0), fresh: true, ..viewer(10_000_000) }];
        assert_eq!(allocate(&ACTIVE, &mut v, u64::MAX), vec![Some(2)]);
    }

    #[test]
    fn budget_downgrades_heaviest_internet_viewers_but_not_lan() {
        let mut v = vec![viewer(10_000_000); 4];
        v[0].lan = true;
        // 3 internet viewers x 4.5 Mbit/s = 13.5 > 8 Mbit/s budget.
        let r = allocate(&ACTIVE, &mut v, 8_000_000);
        assert_eq!(r[0], Some(2), "LAN viewer keeps full quality");
        let total: u64 = r[1..].iter().map(|l| ACTIVE[l.unwrap()].1).sum();
        assert!(total <= 8_000_000, "total {total}");
        assert!(r[1..].contains(&Some(2)), "budget is used, not wasted");
    }

    #[test]
    fn fixed_preference_falls_back_to_lower_active_layer() {
        let active = [(0, 300_000), (1, 1_200_000)];
        let mut v = [Demand { pref: LayerPref::High, ..viewer(100_000) }];
        assert_eq!(allocate(&active, &mut v, u64::MAX), vec![Some(1)]);
    }

    #[test]
    fn rid_mapping() {
        assert_eq!(layer_of_rid(Some("q")), 0);
        assert_eq!(layer_of_rid(Some("h")), 1);
        assert_eq!(layer_of_rid(Some("f")), 2);
        assert_eq!(layer_of_rid(None), 2);
    }

    #[test]
    fn layer_bitrate_measurement() {
        let mut s = LayerSet::default();
        let t0 = Instant::now();
        for i in 0..=30 {
            // 30 fps, 5000 bytes per frame = 1.2 Mbit/s
            s.on_frame(1, 5000, i % 30 == 0, t0 + Duration::from_millis(i * 1000 / 30));
        }
        let b = s.layers[1].bitrate;
        assert!((1_000_000..1_500_000).contains(&b), "bitrate {b}");
        assert!(s.is_active(1, t0 + Duration::from_secs(1)));
        assert!(!s.is_active(1, t0 + Duration::from_secs(3)));
    }
}
