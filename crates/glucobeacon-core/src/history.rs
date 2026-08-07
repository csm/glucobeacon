//! A fixed-capacity window of recent readings.

use heapless::Deque;

use crate::glucose::Glucose;
use crate::reading::Reading;
use crate::time::{Duration, Timestamp};

/// The most recent `N` readings, oldest first.
///
/// The display node uses this for the delta arrow and the trend graph, and the
/// gateway uses it to decide whether a poll returned anything new. Capacity is
/// a const parameter and nothing allocates, so it works on bare metal; `N = 36`
/// is three hours at one reading per five minutes.
#[derive(Clone, Debug)]
pub struct History<const N: usize> {
    readings: Deque<Reading, N>,
}

impl<const N: usize> Default for History<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> History<N> {
    /// An empty history.
    pub const fn new() -> Self {
        Self {
            readings: Deque::new(),
        }
    }

    /// Records a reading, evicting the oldest if the window is full.
    ///
    /// Returns `false` without recording anything if `reading` is a repeat of
    /// the newest measurement or is older than it — polls overlap, and packets
    /// can arrive out of order over the radio.
    pub fn push(&mut self, reading: Reading) -> bool {
        if let Some(newest) = self.latest()
            && reading.at <= newest.at
        {
            return false;
        }
        if self.readings.is_full() {
            let _ = self.readings.pop_front();
        }
        // The window was just made non-full, so this cannot fail.
        let _ = self.readings.push_back(reading);
        true
    }

    /// The most recent reading.
    pub fn latest(&self) -> Option<&Reading> {
        self.readings.back()
    }

    /// The reading before the most recent one.
    pub fn previous(&self) -> Option<&Reading> {
        let mut it = self.readings.iter().rev();
        it.next()?;
        it.next()
    }

    /// Change in mg/dL from the previous reading to the latest one.
    ///
    /// `None` when there are fewer than two readings, or when the gap between
    /// them is longer than `max_gap` — a delta across a two-hour sensor outage
    /// is not a rate of change anybody should act on.
    pub fn delta_mgdl(&self, max_gap: Duration) -> Option<i16> {
        let latest = self.latest()?;
        let previous = self.previous()?;
        if latest.at.saturating_since(previous.at) > max_gap {
            return None;
        }
        Some(latest.glucose.delta_mgdl(previous.glucose))
    }

    /// Readings from oldest to newest.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Reading> {
        self.readings.iter()
    }

    /// Readings taken at or after `since`, oldest first.
    pub fn since(&self, since: Timestamp) -> impl Iterator<Item = &Reading> {
        self.readings.iter().filter(move |r| r.at >= since)
    }

    /// The lowest and highest glucose values in the window.
    pub fn range(&self) -> Option<(Glucose, Glucose)> {
        let mut it = self.readings.iter().map(|r| r.glucose);
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), g| (lo.min(g), hi.max(g))))
    }

    /// How many readings are in the window.
    pub fn len(&self) -> usize {
        self.readings.len()
    }

    /// Whether the window holds no readings.
    pub fn is_empty(&self) -> bool {
        self.readings.is_empty()
    }

    /// Drops every recorded reading.
    pub fn clear(&mut self) {
        self.readings.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trend::Trend;

    fn reading(mgdl: u16, secs: i64) -> Reading {
        Reading::new(
            Glucose::from_mgdl(mgdl),
            Trend::Flat,
            Timestamp::from_unix_secs(secs),
        )
    }

    #[test]
    fn evicts_the_oldest_when_full() {
        let mut h = History::<3>::new();
        for (i, mgdl) in [100u16, 110, 120, 130].into_iter().enumerate() {
            assert!(h.push(reading(mgdl, i as i64 * 300)));
        }
        assert_eq!(h.len(), 3);
        let values: heapless::Vec<u16, 3> = h.iter().map(|r| r.glucose.mgdl()).collect();
        assert_eq!(&values[..], &[110, 120, 130]);
    }

    #[test]
    fn rejects_repeats_and_out_of_order_arrivals() {
        let mut h = History::<8>::new();
        assert!(h.push(reading(100, 300)));
        assert!(!h.push(reading(100, 300)), "same measurement, polled twice");
        assert!(
            !h.push(reading(95, 0)),
            "late packet from before the latest"
        );
        assert!(h.push(reading(105, 600)));
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn delta_needs_two_readings_close_together() {
        let gap = Duration::from_mins(10);
        let mut h = History::<8>::new();
        assert_eq!(h.delta_mgdl(gap), None);

        assert!(h.push(reading(100, 0)));
        assert_eq!(h.delta_mgdl(gap), None, "one reading is not a delta");

        assert!(h.push(reading(112, 300)));
        assert_eq!(h.delta_mgdl(gap), Some(12));

        // A reading an hour later is not five minutes of change.
        assert!(h.push(reading(140, 3_900)));
        assert_eq!(h.delta_mgdl(gap), None);
    }

    #[test]
    fn range_spans_the_window() {
        let mut h = History::<8>::new();
        assert_eq!(h.range(), None);
        for (i, mgdl) in [120u16, 88, 210, 145].into_iter().enumerate() {
            h.push(reading(mgdl, i as i64 * 300));
        }
        assert_eq!(
            h.range(),
            Some((Glucose::from_mgdl(88), Glucose::from_mgdl(210)))
        );
    }

    #[test]
    fn since_filters_by_timestamp() {
        let mut h = History::<8>::new();
        for i in 0..5i64 {
            h.push(reading(100 + i as u16, i * 300));
        }
        let recent: heapless::Vec<i64, 8> = h
            .since(Timestamp::from_unix_secs(900))
            .map(|r| r.at.as_unix_secs())
            .collect();
        assert_eq!(&recent[..], &[900, 1200]);
    }
}
