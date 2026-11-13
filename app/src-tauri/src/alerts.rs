//! Snapshot -> alerts and tray state. Pure (no Tauri), so it is tested here.
//! Texts come from the frontend's translation table (`set_texts`), placeholders are filled here.
use gpw_core::ipc::{CardView, Snapshot};
use gpw_core::policy::Phase;
use std::collections::HashMap;

/// A card that just entered the throttled or alarm phase.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub phase: Phase,
    pub card: String,
    /// 0-based pin and current that caused it.
    pub trigger: Option<(usize, f32)>,
    pub limit_w: Option<f32>,
}

/// What the tray icon shows: worst phase of all cards and the highest pin current.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrayState {
    pub phase: Phase,
    pub max_amps: Option<f32>,
    pub cards: usize,
}

fn rank(p: Phase) -> u8 {
    match p {
        Phase::Ok => 0,
        Phase::Throttled => 1,
        Phase::Alarm => 2,
    }
}

/// Same title as the card in the overview.
pub fn card_title(c: &CardView) -> String {
    c.gpu_name.clone().unwrap_or_else(|| c.source_label.clone())
}

/// Remembers each card's phase (by source id) across snapshots and reconnects.
#[derive(Default)]
pub struct Watch {
    last: HashMap<String, Phase>,
}

impl Watch {
    /// Cards whose phase rose to throttled or alarm since the previous snapshot.
    /// A card seen for the first time counts as coming from "ok".
    pub fn step(&mut self, snap: &Snapshot) -> Vec<Alert> {
        let mut out = Vec::new();
        for c in &snap.cards {
            let before = self.last.insert(c.source_id.clone(), c.phase).unwrap_or(Phase::Ok);
            if c.phase != Phase::Ok && rank(c.phase) > rank(before) {
                out.push(Alert { phase: c.phase, card: card_title(c), trigger: c.trigger, limit_w: c.limit_w });
            }
        }
        out
    }
}

pub fn tray_state(snap: &Snapshot) -> TrayState {
    let phase = snap.cards.iter().map(|c| c.phase).max_by_key(|p| rank(*p)).unwrap_or(Phase::Ok);
    let max_amps = snap
        .cards
        .iter()
        .filter_map(|c| c.amps)
        .flat_map(|a| a.into_iter())
        .filter(|a| a.is_finite())
        .reduce(f32::max);
    TrayState { phase, max_amps, cards: snap.cards.len() }
}

/// Translated templates of the current language (keys as in ui/i18n.js) plus the decimal separator.
#[derive(Default)]
pub struct Texts(pub HashMap<String, String>);

impl Texts {
    fn get(&self, key: &str) -> String {
        self.0.get(key).cloned().unwrap_or_else(|| key.to_string())
    }

    fn fill(&self, key: &str, vals: &[(&str, String)]) -> String {
        let mut s = self.get(key);
        for (k, v) in vals {
            s = s.replace(&format!("{{{k}}}"), v);
        }
        s
    }

    fn num(&self, v: f32, decimals: usize) -> String {
        let s = format!("{v:.decimals$}");
        match self.0.get("dec") {
            Some(d) if d != "." => s.replace('.', d),
            _ => s,
        }
    }

    /// Notification title and body (like the draft's toast: "<card> · <banner text>").
    pub fn notification(&self, a: &Alert) -> (String, String) {
        let (p, amps) = match a.trigger {
            Some((p, amps)) => ((p + 1).to_string(), self.num(amps, 2)),
            None => ("—".into(), "—".into()),
        };
        let w = a.limit_w.map(|w| format!("{}", w.round() as i64)).unwrap_or_else(|| "—".into());
        let (title, banner) = match a.phase {
            Phase::Alarm => ("tAlarm", self.fill("bAlarm", &[("p", p), ("a", amps)])),
            _ => ("tThr", self.fill("bThr", &[("w", w), ("p", p), ("a", amps)])),
        };
        (self.get(title), format!("{} · {banner}", a.card))
    }

    /// Tooltip / tray menu line: state and highest pin, or why there is nothing to show.
    pub fn tray_line(&self, st: Option<TrayState>) -> String {
        let tail = match st {
            None => self.get("noService"),
            Some(s) if s.cards == 0 => self.get("noCards"),
            Some(s) => {
                let key = match s.phase {
                    Phase::Ok => "ok",
                    Phase::Throttled => "thr",
                    Phase::Alarm => "alarm",
                };
                match s.max_amps {
                    Some(a) => format!("{} · {} A", self.get(key), self.num(a, 2)),
                    None => self.get(key),
                }
            }
        };
        format!("Areko GPUPinWatch · {tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpw_core::config::Config;
    use gpw_core::ipc::Binding;
    use gpw_core::model::SourceKind;

    fn card(id: &str, phase: Phase, amps: [f32; 6]) -> CardView {
        CardView {
            source_id: id.into(),
            source_kind: SourceKind::WireView,
            source_label: "WireView Pro II".into(),
            binding: Binding::Matched,
            gpu_pci: Some("0000:02:00.0".into()),
            gpu_name: Some(format!("RTX {id}")),
            amps: Some(amps),
            volts: None,
            power_w: None,
            limit_w: Some(400.0),
            prev_limit_w: None,
            phase,
            trigger: Some((3, 8.62)),
            history_max: vec![],
        }
    }

    fn snap(cards: Vec<CardView>) -> Snapshot {
        Snapshot { version: "0".into(), poll_hz: 2, age_ms: 0, cards, sources: vec![], events: vec![], config: Config::default() }
    }

    #[test]
    fn alerts_only_on_rising_phase_and_tray_shows_worst_card() {
        let calm = [7.0, 7.1, 7.2, 7.0, 6.9, 7.0];
        let mut w = Watch::default();
        assert!(w.step(&snap(vec![card("a", Phase::Ok, calm), card("b", Phase::Ok, calm)])).is_empty());
        let hot = snap(vec![card("a", Phase::Throttled, [5.1, 5.3, 5.2, 8.62, 4.8, 5.3]), card("b", Phase::Ok, calm)]);
        let a = w.step(&hot);
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].phase, a[0].card.as_str()), (Phase::Throttled, "RTX a"));
        assert!(w.step(&hot).is_empty(), "same phase again must not alert twice");
        assert_eq!(w.step(&snap(vec![card("a", Phase::Alarm, calm), card("b", Phase::Ok, calm)]))[0].phase, Phase::Alarm);
        assert!(w.step(&snap(vec![card("a", Phase::Ok, calm)])).is_empty(), "falling back is no alert");

        let t = tray_state(&hot);
        assert_eq!((t.phase, t.max_amps, t.cards), (Phase::Throttled, Some(8.62), 2));
        let texts = Texts([("bThr", "Auf {w} W gedrosselt – Pin {p} lag bei {a} A"), ("tThr", "GPU gedrosselt"), ("dec", ",")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect());
        let (title, body) = texts.notification(&w_alert());
        assert_eq!(title, "GPU gedrosselt");
        assert_eq!(body, "RTX a · Auf 400 W gedrosselt – Pin 4 lag bei 8,62 A");
    }

    fn w_alert() -> Alert {
        Alert { phase: Phase::Throttled, card: "RTX a".into(), trigger: Some((3, 8.62)), limit_w: Some(400.0) }
    }
}
