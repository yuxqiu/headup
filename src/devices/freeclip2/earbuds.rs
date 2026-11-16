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
//!    computer starts streaming, unless an accepted answer still stands. A refused switch is sent
//!    again on every 43/54 saying this computer streams. An accepted one survives audio gaps: the
//!    earbuds resume by themselves, and sending again would restart the stream and add a
//!    re-centre (FINDINGS "Grounding of headup's device logic"). The earbuds answer every 43/88
//!    (firmware `0x7039c018`; 13 of 13 live, within 80 ms), so no retry timer is needed.
//! 3. **An accepted answer is cancelled** by taking an earbud off (no poses after re-wearing until
//!    a new 43/88, data/grounding_test.log "rewear") or by another device starting to stream (no
//!    poses after this computer streams again until a new 43/88, data/grounding_test3.log
//!    "phone_takeover"). Rule 2 then sends it again.
//! 4. **A hold** (the earbuds' accelerometer activity detector locks the reference to the head
//!    while they sense movement: the user walking, or the earbuds handled; FINDINGS "Instant resets
//!    while walking", "A hold after putting the earbuds back in") starts when the rebuilder
//!    recognises it
//!    ([`super::rebuild::Rebuilt::HoldStarted`]) and ends at the first reading that is not
//!    identity, or when the earbuds go back in.

use super::protocol::SwitchReply;
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

/// What the earbuds have said about themselves. `None` until they say it.
#[derive(Default)]
pub(super) struct Earbuds {
    /// 43/37: both earbuds are in the ears.
    worn: Option<bool>,
    /// 43/54 (or a 43/89 reading): this computer streams audio to the earbuds.
    streaming: Option<bool>,
    /// The earbuds' answer to the last "stream on" that still stands. `None`: no answer yet, or an
    /// accepted one they cancelled since (rule 3).
    answer: Option<SwitchReply>,
    /// The earbuds hold (rule 4).
    holding: bool,
}

impl Earbuds {
    /// The link sent "stream on": no answer stands until the earbuds reply.
    pub(super) fn switch_sent(&mut self) {
        self.answer = None;
    }

    /// 43/37. Taking an earbud off cancels an accepted switch (rule 3); putting them back in
    /// starts a new stream, which needs a new switch (rule 2) and ends a hold (rule 4).
    pub(super) fn on_wear(&mut self, worn: bool) -> Option<SwitchOn> {
        let was = self.worn.replace(worn);
        if !worn {
            self.cancel_accepted();
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

    /// 43/54 for one device. Another device starting to stream cancels an accepted switch
    /// (rule 3); this computer streaming may send it again (rule 2).
    pub(super) fn on_audio(&mut self, this_computer: bool, streaming: bool) -> Option<SwitchOn> {
        if !this_computer {
            if streaming {
                self.cancel_accepted();
            }
            return None;
        }
        let started = streaming && self.streaming != Some(true);
        self.streaming = Some(streaming);
        let resend = match self.answer {
            Some(SwitchReply::Accepted) => false,
            Some(SwitchReply::PeerLinkDown | SwitchReply::NotCurrentDevice) => streaming,
            // No answer yet (one may be a refusal still in transit: the 43/54 can arrive first),
            // or a cancelled one: send when this computer starts streaming, not on every push.
            None => started,
        };
        resend.then_some(SwitchOn)
    }

    /// 43/88 reply.
    pub(super) fn on_reply(&mut self, reply: SwitchReply) {
        self.answer = Some(reply);
    }

    /// Whether tracking flows (rule 1). `None` while it is unknown whether this computer streams.
    pub(super) fn availability(&self) -> Option<Availability> {
        let blocker = if self.worn == Some(false) {
            Some(Blocker::NotWorn)
        } else {
            match self.answer {
                Some(SwitchReply::PeerLinkDown) => Some(Blocker::PeerLinkDown),
                Some(SwitchReply::NotCurrentDevice) => Some(Blocker::NotStreaming),
                None | Some(SwitchReply::Accepted) if !self.streaming? => {
                    Some(Blocker::NotStreaming)
                }
                None | Some(SwitchReply::Accepted) if self.holding => Some(Blocker::Moving),
                None | Some(SwitchReply::Accepted) => None,
            }
        };
        Some(blocker.map_or(Availability::Available, Availability::Unavailable))
    }

    /// The earbuds cancelled an accepted switch (rule 3).
    fn cancel_accepted(&mut self) {
        if self.answer == Some(SwitchReply::Accepted) {
            self.answer = None;
        }
    }
}
