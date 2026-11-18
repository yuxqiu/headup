//! What the FreeClip 2 earbuds have said about themselves, and whether head tracking flows from
//! that. No I/O: the link feeds [`Earbuds`] parsed messages, sends what it asks for, and reports
//! its [`Availability`] to the session.
//!
//! The rules:
//!
//! 1. **Tracking flows** when both earbuds are worn, the "stream on" switch is not refused, this
//!    computer streams audio to them, and the earbuds do not hold. Checked in that order; the
//!    first that fails is the [`Blocker`]. A removed earbud stops tracking within about 1 s
//!    (FINDINGS "Single earbud"); the earbuds send poses only to their current device, the last
//!    one to start streaming (FINDINGS "Tracking runs only while this computer streams audio").
//! 2. **"Stream on" (43/88) is sent** at connect, after the earbuds go back in, and when this
//!    computer starts streaming, unless the switch is still on. A refused switch is sent again on
//!    every 43/54 saying this computer streams. A switch that is on survives audio gaps: the
//!    earbuds resume by themselves, and sending again would restart the stream and add a
//!    re-centre (FINDINGS "Grounding of headup's device logic"). The earbuds answer every 43/88
//!    (firmware `0x7039c018`; 13 of 13 live, within 80 ms), so no retry timer is needed.
//! 3. **A switch that is on is dropped** by taking an earbud off (no poses after re-wearing until
//!    a new 43/88, data/grounding_test.log "rewear") or by another device becoming the earbuds'
//!    current device (no poses after this computer streams again until a new 43/88,
//!    data/grounding_test3.log "phone_takeover", where the phone streamed;
//!    data/slouch_retry_raw.jsonl, where it took over with `05`, a call `[inferred]`). A 43/54 or
//!    43/49 entry shows another device as current when its audio state is not quiet (FINDINGS
//!    "43/54 state byte"). Rule 2 then sends it again. An acceptance that arrives after another
//!    device took over still counts as on; the earbuds answer within 80 ms, so this is not
//!    handled.
//! 4. **A hold** (the earbuds' accelerometer activity detector locks the reference to the head
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
    /// The earbuds hold while they sense movement (rule 4).
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

/// A request to send "stream on", from an [`Earbuds`] handler (rule 2).
#[must_use = "the earbuds' state asks for the switch; send it with `Link::send_switch_on`"]
pub(super) struct SwitchOn;

/// Where "stream on" (43/88) stands for this computer (rules 2 and 3). The link sends it at
/// connect, so it starts unconfirmed.
#[derive(Clone, Copy, PartialEq, Default)]
enum Switch {
    /// No acceptance holds: sent and not answered yet, or the earbuds dropped it (rule 3).
    #[default]
    Unconfirmed,
    /// Accepted and still holding: the earbuds resume the stream by themselves across audio gaps.
    On,
    /// Refused; the refusal is what blocks tracking.
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
    /// 43/54 (or a 43/89 reading): this computer streams audio to the earbuds.
    streaming: Option<bool>,
    /// 43/88: where "stream on" stands (rules 2 and 3).
    switch: Switch,
    /// The earbuds hold (rule 4).
    holding: bool,
}

impl Earbuds {
    /// The link sent "stream on".
    pub(super) fn switch_sent(&mut self) {
        self.switch = Switch::Unconfirmed;
    }

    /// 43/37. Taking an earbud off drops the switch (rule 3); putting them back in starts a new
    /// stream, which needs a new switch (rule 2) and ends a hold (rule 4).
    pub(super) fn on_wear(&mut self, worn: bool) -> Option<SwitchOn> {
        let was = self.worn.replace(worn);
        if !worn {
            self.drop_switch();
            return None;
        }
        if was != Some(false) {
            return None;
        }
        self.holding = false;
        Some(SwitchOn)
    }

    /// 43/89: a hold started (rule 4).
    pub(super) fn on_hold_started(&mut self) {
        self.holding = true;
    }

    /// 43/89. A reading also tells that this computer streams, if no 43/54 has said so yet (the
    /// earbuds send none on connect while audio is already playing). A reading that is not
    /// identity ends a hold (rule 4).
    pub(super) fn on_reading(&mut self, raw: Quat) {
        self.streaming.get_or_insert(true);
        if raw != Quat::IDENTITY {
            self.holding = false;
        }
    }

    /// 43/54 or 43/49 for this computer: whether it streams. Starting to stream may send the
    /// switch again (rule 2).
    pub(super) fn on_audio(&mut self, streaming: bool) -> Option<SwitchOn> {
        let started = streaming && self.streaming != Some(true);
        self.streaming = Some(streaming);
        let send = match self.switch {
            Switch::On => false,
            Switch::Refused(_) => streaming,
            // A reply may still be in transit (the 43/54 can arrive first), or the switch was
            // dropped: send when this computer starts streaming, not on every push.
            Switch::Unconfirmed => started,
        };
        send.then_some(SwitchOn)
    }

    /// 43/88 reply.
    pub(super) fn on_reply(&mut self, reply: SwitchReply) {
        self.switch = match reply {
            SwitchReply::Accepted => Switch::On,
            SwitchReply::Refused(refusal) => Switch::Refused(refusal),
        };
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

    /// The earbuds dropped a switch that was on: an earbud came off, or another device became
    /// their current device (rule 3).
    pub(super) fn drop_switch(&mut self) {
        if self.switch == Switch::On {
            self.switch = Switch::Unconfirmed;
        }
    }
}
