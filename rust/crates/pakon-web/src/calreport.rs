//! The light calibration of a scan, read off the scanner's log lines, for
//! the `calibration` usage event: enough to tell on someone else's unit
//! whether a calibration clipped, how it got there and with which A/D gain,
//! LED currents and duties. Numbers only: no serials, no paths.

use serde_json::{json, Map, Value};

/// Refine passes kept from the start of the refine (the rest is counted).
const KEEP_REFINES: usize = 4;

#[derive(Debug, Default)]
pub struct CalReport {
    /// full, refresh or reuse.
    action: Option<String>,
    /// The currents a refresh starts from ("R8 G24 B23 IR6").
    prior: Option<String>,
    /// Current-search passes and the last one ("R8 G24 B23 IR6 -> 65534/...").
    search: usize,
    search_last: Option<String>,
    /// Refine passes ("392/356/110/587 -> 64401/63711/65534/39666").
    refines: Vec<String>,
    refine_n: usize,
    refine_last: Option<String>,
    /// The last dark pass ("offsets 11/3/4 -> black means 288/326/311").
    dark: Option<String>,
    /// Open-gate means of the bright tables, R, G, B, and the IR leak.
    bright: [Option<String>; 3],
    leak: [Option<String>; 3],
    /// The summary line: gains, offsets, currents, scan duties, motor speed.
    result: Option<String>,
    /// Why the calibration failed.
    fail: Option<String>,
    /// A refresh that failed before this full calibration: why, its passes
    /// and its last refine pass.
    refresh_failed: Option<String>,
}

fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.find(key).map(|i| line[i + key.len()..].trim())
}

impl CalReport {
    /// Take one log line (any line; the others are ignored).
    pub fn line(&mut self, line: &str) {
        if line.contains("calibration: refresh failed") {
            // the scan calibrates again from scratch, without another
            // "calibration:" line
            let why = format!(
                "{} after {} passes, last {}",
                self.fail.as_deref().unwrap_or("failed"),
                self.refine_n,
                self.refine_last.as_deref().unwrap_or("none")
            );
            *self = CalReport { action: Some("full".into()), prior: self.prior.take(), refresh_failed: Some(why), ..CalReport::default() };
        } else if let Some(rest) = after(line, "calibration: ") {
            // a new calibration: "full (none saved ...)", "refresh (last ...)"
            *self = CalReport { action: rest.split_whitespace().next().map(str::to_string), ..CalReport::default() };
        } else if let Some(rest) = after(line, "current: from the last calibration, ") {
            self.prior = Some(rest.to_string());
        } else if line.starts_with("current ") {
            self.search += 1;
            self.search_last = line.split_once(": ").map(|(_, r)| r.replace(" -> peaks ", " -> "));
        } else if line.starts_with("refine ") {
            let pass = line.split_once(": ").map(|(_, r)| r.trim_start_matches("duties ").replace(" -> peaks ", " -> "));
            if let Some(p) = pass {
                if self.refines.len() < KEEP_REFINES {
                    self.refines.push(p.clone());
                }
                self.refine_last = Some(p);
            }
            self.refine_n += 1;
        } else if line.starts_with("dark ") {
            self.dark = line.split_once(": ").map(|(_, r)| r.to_string());
        } else if let Some(rest) = after(line, "tables ") {
            let ch = match rest.as_bytes().first() {
                Some(b'R') => 0,
                Some(b'G') => 1,
                Some(b'B') => 2,
                _ => return,
            };
            self.bright[ch] = after(rest, "bright active ").map(str::to_string);
            self.leak[ch] = after(rest, "(IR leak ").map(|s| s.trim_end_matches(|c| c != ')').trim_end_matches(')').to_string());
        } else if let Some(rest) = after(line, "calibrated: ") {
            self.result = Some(rest.to_string());
        } else if let Some(rest) = after(line, "light-cal: ") {
            self.fail = Some(rest.to_string());
        }
    }

    /// The event's properties, or None when no calibration was seen.
    pub fn props(&self) -> Option<Map<String, Value>> {
        let action = self.action.as_ref()?;
        let mut p = Map::new();
        let mut put = |k: &str, v: Value| {
            if !v.is_null() {
                p.insert(k.into(), v);
            }
        };
        let slash = |a: &[Option<String>; 3]| a.iter().all(Option::is_some).then(|| a.iter().flatten().cloned().collect::<Vec<_>>().join("/"));
        put("action", json!(action));
        put("prior", json!(self.prior));
        if self.search > 0 {
            put("search", json!(self.search));
            put("search_last", json!(self.search_last));
        }
        if self.refine_n > 0 {
            put("passes", json!(self.refine_n));
            put("refine", json!(self.refines.join("; ")));
            if self.refine_n > KEEP_REFINES {
                put("refine_last", json!(self.refine_last));
            }
        }
        put("dark", json!(self.dark));
        put("bright", json!(slash(&self.bright)));
        put("ir_leak", json!(slash(&self.leak)));
        put("result", json!(self.result));
        put("fail", json!(self.fail));
        put("refresh_failed", json!(self.refresh_failed));
        Some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The C-41 refresh in pakon.log (F-135+, 2026-11-17 09:58).
    const REFRESH: &str = "\
film: colour negative
lamp: stable, 40.2 C
calibration: refresh (last 909 min ago, last full 909 min ago)
calibration line: 2043 px (register window 2043 px); scan 2000 px from pixel 43, 20 black
dark 1: offsets 11/3/4 -> black means 296/332/313
current: from the last calibration, R8 G24 B23 IR6
refine 1: duties 392/356/110/587 -> peaks 65534/65534/65534/65534
refine 2: duties 392/356/110/358 -> peaks 65534/65534/65534/50131
refine 3: duties 392/356/110/285 -> peaks 65534/65534/65534/41205
refine 4: duties 392/356/110/276 -> peaks 65534/65534/65534/40047
refine 5: duties 392/356/110/276 -> peaks 65534/65534/65534/40028
tables R: 136/137 lines, dark black 296 active 319 (IR leak +23), bright active 65534
tables G: 136/137 lines, dark black 333 active 351 (IR leak +18), bright active 65534
tables B: 136/137 lines, dark black 313 active 330 (IR leak +18), bright active 65534
flat field -> C:\\Users\\someone\\pakon\\000013\\scan.raw.flat.json
calibrated: gains 13/13/13, offsets 11/3/4, currents 8/24/23/6, scan duties 546/597/571/276, motor speed 4954
[sm] motor start (a0) sent -- feed the film now";

    #[test]
    fn a_refresh() {
        let mut c = CalReport::default();
        for l in REFRESH.lines() {
            c.line(l);
        }
        let p = Value::Object(c.props().unwrap());
        assert_eq!(
            p,
            json!({
                "action": "refresh",
                "prior": "R8 G24 B23 IR6",
                "passes": 5,
                "refine": "392/356/110/587 -> 65534/65534/65534/65534; 392/356/110/358 -> 65534/65534/65534/50131; \
                           392/356/110/285 -> 65534/65534/65534/41205; 392/356/110/276 -> 65534/65534/65534/40047",
                "refine_last": "392/356/110/276 -> 65534/65534/65534/40028",
                "dark": "offsets 11/3/4 -> black means 296/332/313",
                "bright": "65534/65534/65534",
                "ir_leak": "+23/+18/+18",
                "result": "gains 13/13/13, offsets 11/3/4, currents 8/24/23/6, scan duties 546/597/571/276, motor speed 4954",
            })
        );
        // no paths, and small enough for the 2048-character event
        assert!(!p.to_string().contains("Users"));
        assert!(p.to_string().len() < 1200);
    }

    #[test]
    fn a_full_one_that_fails_and_none() {
        let mut c = CalReport::default();
        assert!(c.props().is_none());
        for l in ["calibration: full (none saved for bwc41, Base 16)", "current 1: R1 G1 B1 IR1 -> peaks 9331/4514/3839/6845", "light-cal: insufficient light on IR"] {
            c.line(l);
        }
        let p = c.props().unwrap();
        assert_eq!(p["action"], "full");
        assert_eq!(p["search"], 1);
        assert_eq!(p["search_last"], "R1 G1 B1 IR1 -> 9331/4514/3839/6845");
        assert_eq!(p["fail"], "insufficient light on IR");
        assert!(!p.contains_key("bright"));
    }

    #[test]
    fn a_refresh_that_falls_back_to_full() {
        let mut c = CalReport::default();
        for l in [
            "calibration: refresh (last 64 min ago, last full 973 min ago)",
            "current: from the last calibration, R8 G24 B23 IR6",
            "refine 1: duties 392/356/110/587 -> peaks 65534/65534/65534/65534",
            "refine 2: duties 196/178/55/293 -> peaks 40000/41000/39000/30000",
            "light-cal: duties did not converge",
            "calibration: refresh failed, calibrating from scratch",
            "current 1: R1 G1 B1 IR1 -> peaks 9331/4514/3839/6845",
            "calibrated: gains 13/13/13, offsets 11/3/4, currents 8/24/23/6, scan duties 546/597/571/587, motor speed 4954",
        ] {
            c.line(l);
        }
        let p = c.props().unwrap();
        assert_eq!(p["action"], "full");
        assert_eq!(p["prior"], "R8 G24 B23 IR6");
        assert_eq!(p["refresh_failed"], "duties did not converge after 2 passes, last 196/178/55/293 -> 40000/41000/39000/30000");
        assert_eq!(p["search"], 1);
        assert!(!p.contains_key("fail"));
    }
}
