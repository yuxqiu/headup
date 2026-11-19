//! What the FreeClip 2 earbuds have said about themselves, and what follows from it: whether head
//! tracking flows, and whether to ask for it. No I/O: the link feeds [`Earbuds`] parsed messages,
//! then sends "stream on" if [`Earbuds::ask`] says so and reports its [`Availability`] to the
//! session. Handlers only record facts; those two functions decide.
//!
//! The rules:
//!
//! 1. **Tracking flows** when both earbuds are worn, the "stream on" switch is not refused, this
//!    computer streams audio to them, and the earbuds do not hold. Checked in that order; the
//!    first that fails is the [`Blocker`]. A removed earbud stops tracking within about 1 s
//!    (FINDINGS "Single earbud"); the earbuds send poses only to their current device, the last
//!    one to start streaming (FINDINGS "Tracking runs only while this computer streams audio").
//! 2. **"Stream on" (43/88) is asked for once** per stream of this computer's: when it has not
//!    been asked since the earbuds last lost that stream, this computer streams, and the earbuds
//!    are not out. Streaming also shows this computer is the earbuds' current device (FINDINGS
//!    "43/54 state byte"), so the request can take effect; a 43/88 while idle is accepted but
//!    starts nothing (FINDINGS "Asking for the audio state"). Asking again would restart the
//!    stream and add a re-centre (FINDINGS "Grounding of headup's device logic"), and an accepted
//!    stream survives this computer's audio gaps: the earbuds resume by themselves. The earbuds
//!    answer every 43/88 (firmware `0x7039c018`; 13 of 13 live, within 80 ms), so no retry timer
//!    is needed.
//! 3. **The earbuds lose this computer's stream** when an earbud comes off (no poses after
//!    re-wearing until a new 43/88, data/grounding_test.log "rewear") or another device becomes
//!    their current device (no poses after this computer streams again until a new 43/88,
//!    data/grounding_test3.log "phone_takeover", where the phone streamed;
//!    data/slouch_retry_raw.jsonl, where it took over with `05`, a call `[inferred]`). A 43/54 or
//!    43/49 entry shows another device as current when its audio state is not quiet (FINDINGS
//!    "43/54 state byte"). Then the next stream asks again.
//! 4. **An answer holds for the stream it was asked about**, until the earbuds lose that stream.
//!    An acceptance needs no record: having asked is enough. A refusal blocks tracking with its
//!    reason; it is not retried, since no report tells when its cause ends (FINDINGS "Reply value
//!    1": the earbuds' link to each other comes back unseen), and asking only while this computer
//!    streams rules out "not the current device". An answer arriving after the stream was lost
//!    changes nothing.
//! 5. **A hold** (the earbuds' accelerometer activity detector locks the reference to the head
//!    while they sense movement: the user walking, or the earbuds handled; FINDINGS "Instant resets
//!    while walking", "A hold after putting the earbuds back in") starts when the rebuilder
//!    recognises it
//!    ([`super::rebuild::Rebuilt::HoldStarted`]) and ends at the first reading that is not
//!    identity, or when the earbuds go back in.

use super::protocol::{Refusal, SwitchReply};
use crate::core::quat::Quat;

/// Whether head tracking flows, as reported to the session.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Availability {
    Available,
    Unavailable(Blocker),
}

/// Why FreeClip 2 cannot deliver head tracking (rule 1).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Blocker {
    /// An earbud is out (43/37).
    NotWorn,
    /// This computer is not the earbuds' current device (43/54, or 43/88 reply 2).
    NotStreaming,
    /// The two earbuds cannot reach each other (43/88 reply 1).
    PeerLinkDown,
    /// The earbuds hold while they sense movement (rule 5).
    Moving,
}

impl Blocker {
    /// The sentence the session shows and logs.
    pub(super) fn reason(self) -> &'static str {
        match self {
            Blocker::NotWorn => "put both earbuds in; FreeClip 2 tracks your head only with both",
            Blocker::NotStreaming => {
                "play audio on this computer; FreeClip 2 only tracks your head while it is playing"
            }
            // [unverified]: reply 1 has never been recorded; the sentence follows the firmware's
            // meaning (FINDINGS "Reply value 1").
            Blocker::PeerLinkDown => {
                "the two earbuds have lost their link to each other; \
                 FreeClip 2 tracks your head only while they are linked"
            }
            Blocker::Moving => {
                "FreeClip 2 pauses head tracking while it senses movement, such as walking around \
                 or handling the earbuds; it resumes when you move your head after you stop"
            }
        }
    }
}

/// A request to send "stream on", made by [`Earbuds::ask`], which records it as asked (rule 2).
#[must_use = "the earbuds' state asks for the switch; send it"]
pub(super) struct SwitchOn;

/// "Stream on" (43/88) for this computer's stream (rules 2 to 4).
#[derive(Clone, Copy, PartialEq, Default)]
enum Switch {
    /// Not asked since the earbuds last lost this computer's stream.
    #[default]
    Unasked,
    /// Asked: the answer is pending, or it was an acceptance.
    Asked,
    /// Refused: the reason blocks tracking until the stream is lost and asked for again.
    Refused(Refusal),
}

impl Refusal {
    /// What blocks tracking while the earbuds refuse "stream on".
    fn blocker(self) -> Blocker {
        match self {
            Refusal::PeerLinkDown => Blocker::PeerLinkDown,
            Refusal::NotCurrentDevice => Blocker::NotStreaming,
        }
    }
}

/// What the earbuds have said about themselves. `None` until they say it.
#[derive(Default)]
pub(super) struct Earbuds {
    /// 43/37: both earbuds are in the ears.
    worn: Option<bool>,
    /// 43/54 or 43/49 (or a 43/89 reading): this computer streams audio to the earbuds.
    streaming: Option<bool>,
    /// 43/88 (rules 2 to 4).
    switch: Switch,
    /// The earbuds hold (rule 5).
    holding: bool,
}

impl Earbuds {
    /// Whether to send "stream on" now (rule 2). If so, it counts as asked from here on.
    pub(super) fn ask(&mut self) -> Option<SwitchOn> {
        let wanted = self.switch == Switch::Unasked
            && self.streaming == Some(true)
            && self.worn != Some(false);
        if !wanted {
            return None;
        }
        self.switch = Switch::Asked;
        Some(SwitchOn)
    }

    /// 43/37. An earbud coming off loses this computer's stream (rule 3); going back in ends a
    /// hold (rule 5).
    pub(super) fn on_wear(&mut self, worn: bool) {
        let was = self.worn.replace(worn);
        if !worn {
            self.stream_lost();
        } else if was == Some(false) {
            self.holding = false;
        }
    }

    /// 43/89: a hold started (rule 5).
    pub(super) fn on_hold_started(&mut self) {
        self.holding = true;
    }

    /// 43/89. A reading also tells that this computer streams, if no 43/54 has said so yet (the
    /// earbuds send none on connect while audio is already playing). A reading that is not
    /// identity ends a hold (rule 5).
    pub(super) fn on_reading(&mut self, raw: Quat) {
        self.streaming.get_or_insert(true);
        if raw != Quat::IDENTITY {
            self.holding = false;
        }
    }

    /// 43/54 or 43/49 for this computer: whether it streams.
    pub(super) fn on_audio(&mut self, streaming: bool) {
        self.streaming = Some(streaming);
    }

    /// The earbuds lost this computer's stream: an earbud came off, or another device became
    /// their current device (rule 3).
    pub(super) fn stream_lost(&mut self) {
        self.switch = Switch::Unasked;
    }

    /// 43/88 reply. Only a refusal of the request asked for counts (rule 4).
    pub(super) fn on_reply(&mut self, reply: SwitchReply) {
        if let (Switch::Asked, SwitchReply::Refused(refusal)) = (self.switch, reply) {
            self.switch = Switch::Refused(refusal);
        }
    }

    /// Whether tracking flows (rule 1). `None` while it is unknown whether this computer streams.
    pub(super) fn availability(&self) -> Option<Availability> {
        let blocker = if self.worn == Some(false) {
            Some(Blocker::NotWorn)
        } else {
            match self.switch {
                Switch::Refused(refusal) => Some(refusal.blocker()),
                _ if !self.streaming? => Some(Blocker::NotStreaming),
                _ if self.holding => Some(Blocker::Moving),
                _ => None,
            }
        };
        Some(blocker.map_or(Availability::Available, Availability::Unavailable))
    }
}
