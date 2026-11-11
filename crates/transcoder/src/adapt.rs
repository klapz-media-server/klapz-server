//! When an adaptive stream moves to another rung: the decisions, apart from the processes they
//! restart.
//!
//! Every move costs a restart of the transcoder, which throws away what it had produced ahead
//! and makes the player wait for the first segment of the new run. So every rule here leans
//! towards staying put: a player or a link hovering between two rungs must not make the server
//! restart its process back and forth.

use std::time::{Duration, Instant};

/// A segment the player has received whole. `sending` runs from the first byte handed to the
/// connection to the last: the time spent waiting for the process to write it is not in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivery {
    pub bytes: u64,
    pub sending: Duration,
}

/// A send shorter than this mostly filled the connection's buffers, and says how fast the server
/// writes rather than how fast the player reads. Counted at this length, a small segment still
/// reads as a fast link, which is what lets a stream climb back from its lowest rung.
const SHORTEST_SEND: Duration = Duration::from_millis(20);

/// Weights of the newest delivery in the two averages. The fast one follows a link that drops
/// within two segments; the slow one keeps a single lucky segment from reading as a fast link.
const FAST_WEIGHT: f64 = 0.5;
const SLOW_WEIGHT: f64 = 0.15;

/// The link's throughput, in kilobits per second: the lower of a fast and a slow moving average,
/// so that it falls quickly and rises slowly.
#[derive(Debug, Clone, Default)]
struct Throughput {
    fast: Option<f64>,
    slow: Option<f64>,
    latest: Option<f64>,
}

impl Throughput {
    #[allow(
        clippy::cast_precision_loss,
        reason = "a segment's size, far below 2^52 bytes"
    )]
    /// `ceiling`: a segment sent faster says only that the link carries every rung. Counted at
    /// its own speed, a burst of segments taken by the connection's buffers at once would hold
    /// the averages so high that a link that then drops would take a minute to show it.
    fn record(&mut self, delivery: Delivery, ceiling: f64) {
        let seconds = delivery.sending.max(SHORTEST_SEND).as_secs_f64();
        let kbps = (delivery.bytes as f64 * 8.0 / seconds / 1000.0).min(ceiling);
        let blend = |average: Option<f64>, weight: f64| {
            Some(average.map_or(kbps, |average| average + (kbps - average) * weight))
        };
        self.fast = blend(self.fast, FAST_WEIGHT);
        self.slow = blend(self.slow, SLOW_WEIGHT);
        self.latest = Some(kbps);
    }

    fn estimate(&self) -> Option<f64> {
        Some(self.fast?.min(self.slow?))
    }
}

/// A rung is kept while it needs at most this share of the link, and moved to only when it needs
/// at most the second: the gap between the two is the hysteresis, and the margin below the
/// second absorbs a variable bitrate's peaks.
const KEEP_SHARE: f64 = 0.9;
const MOVE_SHARE: f64 = 0.75;

/// A link carrying twice the top rung's needs carries everything the stream can ask of it.
const CEILING_OVER_TOP: f64 = 2.0;

/// One slow segment is a hiccup; two in a row, six seconds of 3-second segments, is a link that
/// dropped. A player holds more than that in its buffer.
const SLOW_DELIVERIES_BEFORE_DOWNGRADE: u32 = 2;

/// After a move, the next decision waits for the new rung's own segments: three of them.
const SETTLE_AFTER_SWITCH: Duration = Duration::from_secs(9);

/// Moving up waits for the link to have carried the next rung's needs for this long. Ten
/// segments: much longer than a downgrade, since being too low costs quality and being too high
/// costs a stall.
const UPGRADE_HOLD: Duration = Duration::from_secs(30);

/// A move up followed by a move down within this long was a failed probe: the link could not
/// carry the rung. Each one doubles the wait before trying that rung again.
const FAILED_PROBE_WINDOW: Duration = Duration::from_secs(60);
const LONGEST_UPGRADE_HOLD: Duration = Duration::from_secs(240);

#[derive(Debug, Clone, Copy, Default)]
struct Probe {
    reached_at: Option<Instant>,
    failures: u32,
}

/// The server's choice of rung for a stream announced to the player as a single one, from the
/// deliveries it measures.
#[derive(Debug, Clone)]
pub struct ServerAdaptation {
    /// What each rung needs, video and audio, in kilobits per second; best first.
    needs: Vec<u32>,
    current: usize,
    throughput: Throughput,
    slow_deliveries: u32,
    /// Since when every delivery has shown room for the rung above.
    room_above_since: Option<Instant>,
    last_switch: Option<Instant>,
    probes: Vec<Probe>,
}

impl ServerAdaptation {
    /// Starts on the best rung, the one a fixed quality would have given.
    #[must_use]
    pub fn new(needs: Vec<u32>) -> Self {
        let probes = vec![Probe::default(); needs.len()];
        Self {
            needs,
            current: 0,
            throughput: Throughput::default(),
            slow_deliveries: 0,
            room_above_since: None,
            last_switch: None,
            probes,
        }
    }

    /// The rung the stream should be on.
    #[must_use]
    pub const fn current(&self) -> usize {
        self.current
    }

    fn upgrade_hold(&self, rung: usize) -> Duration {
        let failures = self.probes.get(rung).map_or(0, |probe| probe.failures);
        UPGRADE_HOLD
            .saturating_mul(1 << failures.min(8))
            .min(LONGEST_UPGRADE_HOLD)
    }

    /// Takes a delivery into account; the rung to move to, when the stream should move.
    pub fn record(&mut self, delivery: Delivery, now: Instant) -> Option<usize> {
        let ceiling = f64::from(self.needs[0]) * CEILING_OVER_TOP;
        self.throughput.record(delivery, ceiling);
        let estimate = self.throughput.estimate()?;
        if self
            .last_switch
            .is_some_and(|at| now.saturating_duration_since(at) < SETTLE_AFTER_SWITCH)
        {
            return None;
        }
        let fits = |rung: usize, share: f64| f64::from(self.needs[rung]) <= estimate * share;

        if !fits(self.current, KEEP_SHARE) {
            self.room_above_since = None;
            self.slow_deliveries += 1;
            if self.slow_deliveries < SLOW_DELIVERIES_BEFORE_DOWNGRADE {
                return None;
            }
            let lowest = self.needs.len() - 1;
            // Where to land is read from the latest segment too: the averages still remember
            // the faster link, and landing on a rung it no longer carries costs another move.
            let latest = self.throughput.latest.unwrap_or(estimate).min(estimate);
            let target = (self.current + 1..self.needs.len())
                .find(|&rung| f64::from(self.needs[rung]) <= latest * MOVE_SHARE)
                .unwrap_or(lowest);
            return (target != self.current).then(|| self.switch(target, now));
        }
        self.slow_deliveries = 0;

        let above = self.current.checked_sub(1)?;
        if !fits(above, MOVE_SHARE) {
            self.room_above_since = None;
            return None;
        }
        let since = *self.room_above_since.get_or_insert(now);
        (now.saturating_duration_since(since) >= self.upgrade_hold(above))
            .then(|| self.switch(above, now))
    }

    fn switch(&mut self, target: usize, now: Instant) -> usize {
        if target < self.current {
            self.probes[target].reached_at = Some(now);
        } else {
            let probe = &mut self.probes[self.current];
            if probe
                .reached_at
                .is_some_and(|at| now.saturating_duration_since(at) < FAILED_PROBE_WINDOW)
            {
                probe.failures += 1;
            }
        }
        self.current = target;
        self.last_switch = Some(now);
        self.slow_deliveries = 0;
        self.room_above_since = None;
        target
    }
}

/// A player that waits longer than this many segment lengths between two requests was not
/// idle: an idle one asks again as it plays, about once a segment.
const DELAYED_REQUEST_FACTOR: f64 = 1.2;

/// How long a segment really took to reach the player. The server's own send time misses
/// whatever the connection's buffers took at once, a whole small segment over a local link;
/// the wait for the player's next request holds it, but also any pause the player makes with a
/// full buffer. So the wait counts only when it is longer than a pause can be: the player is
/// then fetching slower than it plays, the reference's "segments have been delayed".
#[must_use]
pub fn delivery_time(
    sending: Duration,
    until_next_request: Duration,
    segment: Duration,
) -> Duration {
    if until_next_request.as_secs_f64() > segment.as_secs_f64() * DELAYED_REQUEST_FACTOR {
        until_next_request.max(sending)
    } else {
        sending
    }
}

/// Where a server-side move restarts the process, given the segment the player asks for and the
/// last one the running process has finished. Down at once, since the player is short of data;
/// up one segment later when the one asked for is written, which gives the new process a
/// segment's time to start. Not after all that is written: an encode at a low rung runs minutes
/// ahead of the player.
#[must_use]
pub fn server_restart_at(requested: u32, finished: Option<u32>, upgrade: bool) -> u32 {
    match finished {
        Some(finished) if upgrade && finished >= requested => requested + 1,
        _ => requested,
    }
}

/// Restarts for a change of rung asked by the player are at least this far apart: a player
/// hovering between two rungs gets one restart every few seconds rather than one per request,
/// and each run lives long enough to write a segment or two before it can be replaced.
pub const CLIENT_SWITCH_INTERVAL: Duration = Duration::from_secs(4);

/// A request for a better rung is honored once it has been waiting this long: a player that
/// changes its mind, or aborts the request, has not cost a restart. A worse rung is honored at
/// once, since a player moving down is short of data.
pub const CLIENT_UPGRADE_PERSISTENCE: Duration = Duration::from_secs(2);

/// How much longer a request for a rung other than the running one waits before the process is
/// restarted for it; `None` to restart now. Segments of that rung already on disk are served
/// without asking this.
#[must_use]
pub fn client_switch_wait(
    now: Instant,
    asked_at: Instant,
    last_switch: Option<Instant>,
    upgrade: bool,
) -> Option<Duration> {
    let mut ready = asked_at;
    if upgrade {
        ready += CLIENT_UPGRADE_PERSISTENCE;
    }
    if let Some(last) = last_switch {
        ready = ready.max(last + CLIENT_SWITCH_INTERVAL);
    }
    Some(ready.saturating_duration_since(now)).filter(|wait| !wait.is_zero())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs of a 1080p server ladder: 20 Mbps, then 7.4, 3.4, 1.3 Mbps, 300 and 196 kbps, each
    /// with 256 kbps of audio.
    fn ladder() -> ServerAdaptation {
        ServerAdaptation::new(vec![20_256, 7649, 3675, 1553, 556, 452])
    }

    /// A 3-second segment of `segment_kbps` received over a link of `link_kbps`.
    fn over(link_kbps: u32, segment_kbps: u32) -> Delivery {
        let bytes = u64::from(segment_kbps) * 1000 * 3 / 8;
        Delivery {
            bytes,
            sending: Duration::from_secs_f64(f64::from(segment_kbps) * 3.0 / f64::from(link_kbps)),
        }
    }

    /// Feeds one delivery every three seconds; the moves made, with their time.
    fn play(
        adaptation: &mut ServerAdaptation,
        start: Instant,
        seconds: u64,
        delivery: impl Fn(usize) -> Delivery,
    ) -> Vec<(u64, usize)> {
        let mut moves = Vec::new();
        for at in (0..seconds).step_by(3) {
            let rung = adaptation.current();
            if let Some(target) = adaptation.record(delivery(rung), start + Duration::from_secs(at))
            {
                moves.push((at, target));
            }
        }
        moves
    }

    const SEGMENT_KBPS: [u32; 6] = [20_000, 7393, 3419, 1297, 300, 196];

    #[test]
    fn a_link_that_carries_the_best_rung_stays_on_it() {
        let mut adaptation = ladder();
        let moves = play(&mut adaptation, Instant::now(), 300, |rung| {
            over(100_000, SEGMENT_KBPS[rung])
        });
        assert!(moves.is_empty(), "{moves:?}");
    }

    #[test]
    fn a_dropped_link_moves_down_after_two_slow_segments_to_what_it_carries() {
        let mut adaptation = ladder();
        let moves = play(&mut adaptation, Instant::now(), 30, |rung| {
            over(5000, SEGMENT_KBPS[rung])
        });
        // 7.6 Mbps needs more than three quarters of 5 Mbps; 3.7 Mbps does not.
        assert_eq!(moves, [(3, 2)]);
    }

    #[test]
    fn a_link_dropping_after_segments_sent_at_once_shows_within_three() {
        // Measured over loopback: 1.3 MB segments taken in a millisecond, then a link of
        // 1.5 Mbps reading each in 7 seconds.
        let mut adaptation = ladder();
        let start = Instant::now();
        let instant = Delivery {
            bytes: 1_300_000,
            sending: Duration::from_millis(1),
        };
        assert!(play(&mut adaptation, start, 30, |_| instant).is_empty());
        let slow = Delivery {
            bytes: 1_300_000,
            sending: Duration::from_secs(7),
        };
        let moves = play(&mut adaptation, start + Duration::from_secs(30), 9, |_| {
            slow
        });
        assert_eq!(moves.len(), 1, "{moves:?}");
        assert_eq!(moves[0].1, 4);
    }

    #[test]
    fn a_single_slow_segment_moves_nothing() {
        let mut adaptation = ladder();
        let start = Instant::now();
        assert_eq!(adaptation.record(over(100_000, 20_000), start), None);
        assert_eq!(
            adaptation.record(over(5000, 20_000), start + Duration::from_secs(3)),
            None
        );
        assert_eq!(
            adaptation.record(over(100_000, 20_000), start + Duration::from_secs(6)),
            None
        );
        assert_eq!(adaptation.current(), 0);
    }

    #[test]
    fn moves_up_one_rung_at_a_time_after_thirty_seconds_of_room() {
        let mut adaptation = ladder();
        let start = Instant::now();
        let slow = play(&mut adaptation, start, 6, |rung| {
            over(500, SEGMENT_KBPS[rung])
        });
        assert_eq!(slow, [(3, 5)]);
        let later = start + Duration::from_secs(6);
        let moves = play(&mut adaptation, later, 300, |rung| {
            over(100_000, SEGMENT_KBPS[rung])
        });
        let times: Vec<u64> = moves.iter().map(|(at, _)| *at).collect();
        let rungs: Vec<usize> = moves.iter().map(|(_, rung)| *rung).collect();
        assert_eq!(rungs, [4, 3, 2, 1, 0]);
        // The slow average climbs for a few segments, then each step holds thirty seconds.
        for pair in times.windows(2) {
            assert!(pair[1] - pair[0] >= 30, "{times:?}");
        }
    }

    #[test]
    fn nothing_moves_while_the_new_rung_settles() {
        let mut adaptation = ladder();
        let start = Instant::now();
        assert_eq!(
            play(&mut adaptation, start, 6, |r| over(500, SEGMENT_KBPS[r])),
            [(3, 5)]
        );
        // Even a worse link right after the move changes nothing for nine seconds.
        let soon = start + Duration::from_secs(5);
        assert_eq!(adaptation.record(over(50, 196), soon), None);
    }

    #[test]
    fn a_failed_probe_doubles_the_wait_before_the_next_one() {
        // A link that carries 3.7 Mbps comfortably but 7.6 only until it is used: the stream probes rung 1 and
        // falls back, each probe later than the one before.
        let mut adaptation = ladder();
        let start = Instant::now();
        let moves = play(&mut adaptation, start, 600, |rung| {
            if rung <= 1 {
                over(6000, SEGMENT_KBPS[rung])
            } else {
                over(12_000, SEGMENT_KBPS[rung])
            }
        });
        let probes: Vec<u64> = moves
            .iter()
            .filter(|(_, rung)| *rung == 1)
            .map(|(at, _)| *at)
            .collect();
        assert!(probes.len() >= 3, "{moves:?}");
        let gaps: Vec<u64> = probes.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(gaps.windows(2).all(|pair| pair[1] > pair[0]), "{moves:?}");
    }

    #[test]
    fn small_segments_still_let_the_stream_climb() {
        // 196 kbps for 3 seconds is 73 KB, sent within the connection's buffers.
        let mut adaptation = ladder();
        let start = Instant::now();
        play(&mut adaptation, start, 6, |r| over(500, SEGMENT_KBPS[r]));
        let instant = Delivery {
            bytes: 73_500,
            sending: Duration::from_millis(1),
        };
        assert_eq!(adaptation.current(), 5);
        let moves = play(&mut adaptation, start + Duration::from_secs(6), 60, |_| {
            instant
        });
        assert_eq!(moves.first().map(|(_, rung)| *rung), Some(4));
    }

    #[test]
    fn a_segment_sent_at_once_still_counts_the_wait_of_a_player_fetching_slower_than_it_plays() {
        let segment = Duration::from_secs(3);
        let sent = Duration::from_millis(400);
        // Measured over loopback: 1.3 MB handed to the connection in 0.4 s, read in 5.
        assert_eq!(
            delivery_time(sent, Duration::from_secs(5), segment),
            Duration::from_secs(5)
        );
        // A player asking again after a segment's length was playing, not waiting.
        assert_eq!(delivery_time(sent, Duration::from_secs(3), segment), sent);
        assert_eq!(
            delivery_time(sent, Duration::from_millis(3500), segment),
            sent
        );
    }

    #[test]
    fn a_server_move_down_restarts_at_once_and_up_after_what_is_written() {
        assert_eq!(server_restart_at(40, Some(46), false), 40);
        assert_eq!(server_restart_at(40, Some(46), true), 41);
        assert_eq!(server_restart_at(40, Some(540), true), 41);
        assert_eq!(server_restart_at(40, None, true), 40);
        assert_eq!(server_restart_at(50, Some(46), true), 50);
    }

    #[test]
    fn a_player_moving_down_is_served_at_once_unless_a_restart_was_just_made() {
        let long_ago = Instant::now();
        let now = long_ago + Duration::from_secs(60);
        assert_eq!(client_switch_wait(now, now, None, false), None);
        let just = now.checked_sub(Duration::from_secs(1));
        assert_eq!(
            client_switch_wait(now, now, just, false),
            Some(Duration::from_secs(3))
        );
        assert_eq!(client_switch_wait(now, now, Some(long_ago), false), None);
    }

    #[test]
    fn a_player_moving_up_is_served_once_its_request_has_persisted() {
        let asked = Instant::now();
        assert_eq!(
            client_switch_wait(asked, asked, None, true),
            Some(CLIENT_UPGRADE_PERSISTENCE)
        );
        let now = asked + CLIENT_UPGRADE_PERSISTENCE;
        assert_eq!(client_switch_wait(now, asked, None, true), None);
        // Persisting is not enough within the interval of the last restart.
        let just = asked + Duration::from_secs(1);
        assert_eq!(
            client_switch_wait(now, asked, Some(just), true),
            Some(Duration::from_secs(3))
        );
    }
}
