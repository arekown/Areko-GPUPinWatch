//! User settings. Stored by the service (Linux: /etc/areko-gpupinwatch/config.toml,
//! Windows: %ProgramData%\Areko GPUPinWatch\config.toml); the app changes them via IPC.
//! `sanitize` keeps every value inside safe bounds, whatever a client sends.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum Target {
    /// Absolute power limit in W (clamped to the card's min/max limit).
    Watts(u16),
    /// Percent of the card's default limit.
    Percent(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TripAction {
    Shutdown,
    AlarmOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage1 {
    pub warn_a: f32,
    pub hold_ms: u64,
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage2 {
    pub crit_a: f32,
    pub hold_ms: u64,
    pub action: TripAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub stage1: Stage1,
    pub stage2: Stage2,
    /// Time the lower limit gets to take effect before stage 2 is evaluated.
    pub settle_ms: u64,
    /// 1, 2 or 4 readings per second.
    pub poll_hz: u8,
    pub desktop_alarm: bool,
    pub sound: bool,
    /// App appearance: "light" | "dark" | "areko".
    pub theme: String,
    /// "de" | "en" | "it" | "es" | "pt" | "zh" | "ja" | "" (= system language).
    pub lang: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            stage1: Stage1 { warn_a: 8.5, hold_ms: 1000, target: Target::Watts(400) },
            stage2: Stage2 { crit_a: 9.2, hold_ms: 1500, action: TripAction::Shutdown },
            settle_ms: 2000,
            poll_hz: 2,
            desktop_alarm: true,
            sound: true,
            theme: "areko".into(),
            lang: String::new(),
        }
    }
}

pub const THEMES: [&str; 3] = ["light", "dark", "areko"];
pub const LANGS: [&str; 7] = ["de", "en", "it", "es", "pt", "zh", "ja"];

impl Config {
    /// Clamp everything into safe bounds (same ranges as the settings page).
    pub fn sanitize(mut self) -> Self {
        let f = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.stage1.warn_a = f(self.stage1.warn_a, 7.0, 9.2, 8.5);
        self.stage2.crit_a = f(self.stage2.crit_a, 8.5, 10.0, 9.2).max(self.stage1.warn_a);
        self.stage1.hold_ms = self.stage1.hold_ms.clamp(500, 5000);
        self.stage2.hold_ms = self.stage2.hold_ms.clamp(500, 3000);
        self.settle_ms = self.settle_ms.clamp(500, 5000);
        self.stage1.target = match self.stage1.target {
            Target::Watts(w) => Target::Watts(w.clamp(150, 600)),
            Target::Percent(p) => Target::Percent(p.clamp(50, 95)),
        };
        if ![1, 2, 4].contains(&self.poll_hz) {
            self.poll_hz = 2;
        }
        if !THEMES.contains(&self.theme.as_str()) {
            self.theme = "areko".into();
        }
        if !self.lang.is_empty() && !LANGS.contains(&self.lang.as_str()) {
            self.lang.clear();
        }
        self
    }

    /// Power limit to set when throttling, clamped to what the card accepts.
    pub fn throttle_watts(&self, default_w: Option<f32>, min_w: Option<f32>, max_w: Option<f32>) -> f32 {
        let w = match self.stage1.target {
            Target::Watts(w) => w as f32,
            Target::Percent(p) => default_w.unwrap_or(575.0) * p as f32 / 100.0,
        };
        w.max(min_w.unwrap_or(0.0)).min(max_w.unwrap_or(f32::MAX))
    }

    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        Ok(toml::from_str::<Config>(s)?.sanitize())
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_protection_inside_bounds() {
        let mut c = Config::default();
        c.stage1.warn_a = 50.0;
        c.stage2.crit_a = 3.0;
        c.poll_hz = 9;
        c.theme = "pink".into();
        let c = c.sanitize();
        assert_eq!(c.stage1.warn_a, 9.2);
        assert_eq!(c.stage2.crit_a, 9.2); // never below stage 1
        assert_eq!(c.poll_hz, 2);
        assert_eq!(c.theme, "areko");
        assert_eq!(Config::from_toml(&Config::default().to_toml()).unwrap(), Config::default());
        // 400 W requested, card accepts 400..600
        assert_eq!(Config::default().throttle_watts(Some(600.0), Some(400.0), Some(600.0)), 400.0);
    }
}
