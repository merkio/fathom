//! State and timing of LLM inline suggestions ("ghost text").
//!
//! Pure logic with an injected clock: after the user stops typing for `DEBOUNCE` a request is
//! sent; any further keystroke makes the request stale, cancels it and hides the ghost text.
//! The privacy switch itself is enforced in the use case; this module only decides *when*.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use application::llm::StatementKind;
use application::usecases::InlineSettings;

use crate::bridge::{AiId, TabId};

/// Quiet time after the last keystroke before a request is sent.
pub const DEBOUNCE: Duration = Duration::from_millis(400);

/// Where the user is: tab, cursor and a hash of the text. Two equal snapshots mean "nothing
/// changed", so a suggestion made for one is still valid for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    tab: TabId,
    cursor: usize,
    text_hash: u64,
}

impl Snapshot {
    pub fn new(tab: TabId, cursor: usize, text: &str) -> Self {
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        Self {
            tab,
            cursor,
            text_hash: h.finish(),
        }
    }
}

/// A suggestion ready to show at the cursor it was made for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ghost {
    pub text: String,
    pub safety: Option<StatementKind>,
    at: Snapshot,
}

#[derive(Debug, Default)]
pub struct InlineState {
    pub settings: InlineSettings,
    /// The last error of an inline request, shown in the settings (never as a popup).
    pub error: Option<String>,
    ghost: Option<Ghost>,
    pending: Option<(Snapshot, Instant)>,
    in_flight: Option<(AiId, Snapshot)>,
    requested: Option<Snapshot>,
    next_id: u64,
}

/// What the caller must do after a tick.
#[derive(Debug, PartialEq, Eq)]
pub struct Tick {
    /// Cancel this stale request.
    pub cancel: Option<AiId>,
    /// Send a request for this snapshot with this id.
    pub request: Option<AiId>,
    /// Check again after this long (debounce timer).
    pub wake_in: Option<Duration>,
}

impl Tick {
    fn idle() -> Self {
        Self {
            cancel: None,
            request: None,
            wake_in: None,
        }
    }
}

impl InlineState {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            ..Self::default()
        }
    }

    /// Whether the editor may ask: the switch (or its per-connection override) is on and the
    /// cursor sits where ghost text cannot overlap existing text.
    pub fn eligible(text: &str, cursor: usize) -> bool {
        let Some(before) = text.get(..cursor) else {
            return false;
        };
        let after = &text[cursor..];
        let rest_of_line = after.split('\n').next().unwrap_or_default();
        !before.trim().is_empty() && rest_of_line.trim().is_empty()
    }

    /// Drives the debounce. Call it every frame the editor is focused (and with `enabled =
    /// false` otherwise, which clears everything and cancels what is running).
    pub fn tick(
        &mut self,
        now: Instant,
        enabled: bool,
        snapshot: Snapshot,
        eligible: bool,
    ) -> Tick {
        let mut tick = Tick::idle();
        if !enabled || !eligible {
            tick.cancel = self.reset();
            return tick;
        }
        match self.pending {
            Some((seen, _)) if seen == snapshot => {}
            _ => {
                // Something changed: whatever was suggested or running is stale.
                tick.cancel = self.in_flight.take().map(|(id, _)| id);
                self.ghost = None;
                self.requested = None;
                self.pending = Some((snapshot, now));
                tick.wake_in = Some(DEBOUNCE);
                return tick;
            }
        }
        if self.requested == Some(snapshot) {
            return tick;
        }
        let since = self.pending.map_or(now, |(_, at)| at);
        let waited = now.saturating_duration_since(since);
        if waited < DEBOUNCE {
            tick.wake_in = Some(DEBOUNCE - waited);
            return tick;
        }
        let id = AiId(self.next_id);
        self.next_id += 1;
        self.requested = Some(snapshot);
        self.in_flight = Some((id, snapshot));
        tick.request = Some(id);
        tick
    }

    /// Forgets everything; returns the request to cancel, if one is running.
    pub fn reset(&mut self) -> Option<AiId> {
        self.ghost = None;
        self.pending = None;
        self.requested = None;
        self.in_flight.take().map(|(id, _)| id)
    }

    /// A growing completion arrived. Ignored when it belongs to a stale request.
    pub fn on_text(&mut self, id: AiId, text: String, safety: Option<StatementKind>) {
        if let Some((current, at)) = self.in_flight
            && current == id
        {
            self.ghost = Some(Ghost { text, safety, at });
            self.error = None;
        }
    }

    pub fn on_done(&mut self, id: AiId, error: Option<String>) {
        if self.in_flight.is_some_and(|(current, _)| current == id) {
            // The ghost text (if any) stays; only the request is over.
            self.in_flight = self.in_flight.filter(|(current, _)| *current != id);
            if error.is_some() {
                self.error = error;
            }
        }
    }

    /// The ghost text, if it was made for exactly this tab, cursor and text.
    pub fn ghost_for(&self, snapshot: Snapshot) -> Option<&Ghost> {
        self.ghost.as_ref().filter(|g| g.at == snapshot)
    }

    pub fn take_ghost(&mut self, snapshot: Snapshot) -> Option<Ghost> {
        if self.ghost.as_ref().is_some_and(|g| g.at == snapshot) {
            self.pending = None;
            self.requested = None;
            return self.ghost.take();
        }
        None
    }

    pub fn is_running(&self) -> bool {
        self.in_flight.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(cursor: usize, text: &str) -> Snapshot {
        Snapshot::new(TabId(1), cursor, text)
    }

    fn state() -> InlineState {
        InlineState::new()
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn a_request_goes_out_only_after_the_quiet_period() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(5, "SELEC");

        let first = s.tick(t0, true, a, true);
        assert_eq!((first.request, first.wake_in), (None, Some(DEBOUNCE)));
        let early = s.tick(t0 + MS(399), true, a, true);
        assert_eq!(early.request, None);
        assert_eq!(early.wake_in, Some(MS(1)));

        let due = s.tick(t0 + MS(400), true, a, true);
        assert_eq!(due.request, Some(AiId(1)));
        assert!(s.is_running());
        // Nothing more for the same position while it is answered.
        assert_eq!(s.tick(t0 + MS(900), true, a, true), Tick::idle());
    }

    #[test]
    fn typing_again_cancels_the_stale_request_and_restarts_the_timer() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(5, "SELEC");
        s.tick(t0, true, a, true);
        let id = s
            .tick(t0 + MS(400), true, a, true)
            .request
            .expect("request");
        s.on_text(id, "T".into(), None);
        assert!(s.ghost_for(a).is_some());

        let b = snap(6, "SELECT");
        let tick = s.tick(t0 + MS(450), true, b, true);
        assert_eq!(tick.cancel, Some(id), "the stale request is cancelled");
        assert_eq!(tick.request, None);
        assert!(
            s.ghost_for(a).is_none() && s.ghost_for(b).is_none(),
            "ghost text is hidden at once"
        );
        assert!(!s.is_running());

        // The timer restarted from the new keystroke, not the old one.
        assert_eq!(s.tick(t0 + MS(849), true, b, true).request, None);
        assert!(s.tick(t0 + MS(850), true, b, true).request.is_some());
    }

    #[test]
    fn continuous_typing_never_sends_anything() {
        let mut s = state();
        let t0 = Instant::now();
        let mut text = String::new();
        let mut sent = 0;
        for i in 0..50u64 {
            text.push('a');
            let t = s.tick(t0 + MS(i * 100), true, snap(text.len(), &text), true);
            sent += usize::from(t.request.is_some());
        }
        assert_eq!(
            sent, 0,
            "a keystroke every 100 ms stays inside the debounce window"
        );
    }

    #[test]
    fn nothing_is_requested_while_disabled_and_a_running_request_is_cancelled_when_turned_off() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(5, "SELEC");
        for i in 0..10u64 {
            assert_eq!(
                s.tick(t0 + MS(i * 500), false, a, true),
                Tick::idle(),
                "disabled: no request, ever"
            );
        }
        s.tick(t0, true, a, true);
        let id = s
            .tick(t0 + MS(400), true, a, true)
            .request
            .expect("request");
        let off = s.tick(t0 + MS(500), false, a, true);
        assert_eq!(off.cancel, Some(id));
        assert!(!s.is_running());
    }

    #[test]
    fn ineligible_positions_send_nothing() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(3, "SEL");
        s.tick(t0, true, a, false);
        assert_eq!(s.tick(t0 + MS(1000), true, a, false), Tick::idle());
    }

    #[test]
    fn eligibility_needs_text_before_the_cursor_and_a_blank_rest_of_the_line() {
        assert!(InlineState::eligible("SELECT * FROM ", 14));
        assert!(InlineState::eligible("SELECT *\nFROM", 13));
        assert!(
            InlineState::eligible("SELECT * FROM \nWHERE 1", 14),
            "other lines may follow"
        );
        assert!(!InlineState::eligible("", 0));
        assert!(!InlineState::eligible("   ", 3), "nothing typed yet");
        assert!(
            !InlineState::eligible("SELECT  FROM t", 7),
            "text follows on this line"
        );
        assert!(!InlineState::eligible("é", 1), "not a char boundary");
    }

    #[test]
    fn stale_results_are_ignored() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(5, "SELEC");
        s.tick(t0, true, a, true);
        let old = s
            .tick(t0 + MS(400), true, a, true)
            .request
            .expect("request");
        let b = snap(6, "SELECT");
        s.tick(t0 + MS(500), true, b, true);

        s.on_text(old, "late answer".into(), None);
        assert!(s.ghost_for(a).is_none() && s.ghost_for(b).is_none());
        s.on_done(old, Some("boom".into()));
        assert_eq!(s.error, None, "an old request cannot report errors");
    }

    #[test]
    fn the_ghost_is_valid_only_for_the_exact_position_it_was_made_for() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(14, "SELECT * FROM ");
        s.tick(t0, true, a, true);
        let id = s
            .tick(t0 + MS(400), true, a, true)
            .request
            .expect("request");
        s.on_text(id, "customers".into(), Some(StatementKind::Read));
        assert_eq!(s.ghost_for(a).map(|g| g.text.as_str()), Some("customers"));
        assert!(
            s.ghost_for(snap(13, "SELECT * FROM ")).is_none(),
            "cursor moved"
        );
        assert!(
            s.ghost_for(snap(14, "SELECT * FROM  ")).is_none(),
            "text changed"
        );
        assert!(
            s.ghost_for(Snapshot::new(TabId(2), 14, "SELECT * FROM "))
                .is_none(),
            "other tab"
        );
    }

    #[test]
    fn the_ghost_grows_while_streaming_and_survives_the_end_of_the_request() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(14, "SELECT * FROM ");
        s.tick(t0, true, a, true);
        let id = s
            .tick(t0 + MS(400), true, a, true)
            .request
            .expect("request");
        s.on_text(id, "cust".into(), None);
        s.on_text(id, "customers".into(), None);
        assert_eq!(s.ghost_for(a).map(|g| g.text.as_str()), Some("customers"));
        s.on_done(id, None);
        assert!(!s.is_running());
        assert!(
            s.ghost_for(a).is_some(),
            "still shown after the request ends"
        );
    }

    #[test]
    fn errors_are_kept_for_the_settings_and_cleared_by_the_next_success() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(5, "SELEC");
        s.tick(t0, true, a, true);
        let id = s
            .tick(t0 + MS(400), true, a, true)
            .request
            .expect("request");
        s.on_done(id, Some("rate limited".into()));
        assert_eq!(s.error.as_deref(), Some("rate limited"));

        let b = snap(6, "SELECT");
        s.tick(t0 + MS(500), true, b, true);
        let id = s
            .tick(t0 + MS(900), true, b, true)
            .request
            .expect("request");
        s.on_text(id, "x".into(), None);
        assert_eq!(s.error, None);
    }

    #[test]
    fn accepting_takes_the_ghost_once_and_restarts_the_cycle() {
        let mut s = state();
        let t0 = Instant::now();
        let a = snap(14, "SELECT * FROM ");
        s.tick(t0, true, a, true);
        let id = s
            .tick(t0 + MS(400), true, a, true)
            .request
            .expect("request");
        s.on_text(id, "customers".into(), None);

        assert_eq!(
            s.take_ghost(snap(13, "SELECT * FROM ")),
            None,
            "wrong position"
        );
        let taken = s.take_ghost(a).expect("ghost");
        assert_eq!(taken.text, "customers");
        assert_eq!(s.take_ghost(a), None, "only once");
        // After inserting, the new text starts a new debounce cycle.
        let after = snap(23, "SELECT * FROM customers");
        assert_eq!(
            s.tick(t0 + MS(500), true, after, true).wake_in,
            Some(DEBOUNCE)
        );
    }

    #[test]
    fn request_ids_are_unique() {
        let mut s = state();
        let t0 = Instant::now();
        let mut ids = Vec::new();
        for i in 0..3u64 {
            let sn = snap(i as usize + 1, &"a".repeat(i as usize + 1));
            s.tick(t0 + MS(i * 1000), true, sn, true);
            ids.extend(s.tick(t0 + MS(i * 1000 + 400), true, sn, true).request);
        }
        assert_eq!(ids, [AiId(1), AiId(2), AiId(3)]);
    }
}
