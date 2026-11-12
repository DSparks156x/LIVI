// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The SDLink firmware screen: what carpid says is installed on the RNS-E and the Teensy, what
//! this release bundles, and starting carpid's probe and install jobs. carpid decides every
//! comparison and whether an install may start; this UI only shows it (FIRMWARE.md).

use serde::{Deserialize, Serialize};

use crate::client::FirmwareJob;
use crate::view::{Act, Layout, Row, RowKind, View};

/// A display identity from an installed header/handshake or a bundled manifest.
/// Neither field is ordered by this UI; build IDs may be hashes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Identity {
    pub version: String,
    pub build_id: Option<String>,
}

impl Identity {
    pub fn label(&self) -> String {
        match (self.version.trim(), self.build_id.as_deref()) {
            ("", Some(id)) if !id.is_empty() => id.to_string(),
            ("", _) => "Unknown".into(),
            (version, Some(id)) if !id.is_empty() => format!("{version} ({id})"),
            (version, _) => version.into(),
        }
    }

    fn known(&self) -> bool {
        !self.version.trim().is_empty()
            || self
                .build_id
                .as_ref()
                .is_some_and(|id| !id.trim().is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Installed {
    #[default]
    Unknown,
    /// Explicitly discovered older firmware without a version header, not a failed probe.
    Legacy,
    Reported(Identity),
}

impl Installed {
    pub fn label(&self) -> String {
        match self {
            Self::Unknown => "Unknown".into(),
            Self::Legacy => "Legacy firmware; version unavailable".into(),
            Self::Reported(identity) => identity.label(),
        }
    }
}

/// The backend's authoritative relationship of the bundled image to the installed image.
/// Version strings, hashes and required table versions are never compared in the UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    #[default]
    Unknown,
    Newer,
    Same,
    Older,
    Incompatible,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Target {
    pub installed: Installed,
    pub bundled: Option<Identity>,
    pub comparison: Comparison,
    /// carpid's one line about the installed side (table, formats, where the identity came from).
    pub detail: Option<String>,
    /// `None`: carpid would start an install now. Otherwise why it won't.
    pub blocked: Option<String>,
}

impl Target {
    pub fn update_label(&self) -> &'static str {
        let Some(bundle) = &self.bundled else {
            return "No bundle";
        };
        if !bundle.known() {
            return "Bundle unknown";
        }
        // A partial or stale status must not create an update offer.
        if matches!(&self.installed, Installed::Unknown)
            || matches!(&self.installed, Installed::Reported(id) if !id.known())
        {
            return "Installed unknown";
        }
        match self.comparison {
            Comparison::Unknown => "Unknown",
            Comparison::Newer => "Update available",
            Comparison::Same => "Up to date",
            Comparison::Older => "Newer than bundle",
            Comparison::Incompatible => "Bundle incompatible",
        }
    }

    /// The bundle is a known newer build of a known installed one.
    pub fn update_available(&self) -> bool {
        self.update_label() == "Update available"
    }

    /// An install button is offered: a bundled image, carpid not blocking, no firmware job
    /// running, and carpid hasn't said the bundle is the same or can't run here.
    pub fn installable(&self, busy: bool) -> bool {
        self.bundled.is_some()
            && self.blocked.is_none()
            && !busy
            && !matches!(self.comparison, Comparison::Same | Comparison::Incompatible)
    }
}

/// The protocol in use on this connection, independent of either firmware's identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    #[default]
    Unknown,
    Legacy,
    Negotiated(String),
}

impl Protocol {
    pub fn label(&self) -> String {
        match self {
            Self::Unknown => "Unavailable".into(),
            Self::Legacy => "Legacy protocol".into(),
            Self::Negotiated(version) if !version.trim().is_empty() => {
                format!("Negotiated {version}")
            }
            Self::Negotiated(_) => "Unavailable".into(),
        }
    }
}

/// An uploaded `[prefix]sector_<N>.bin` for Advanced: Flash Sector.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Upload {
    pub name: String,
    pub sector: u16,
    pub size: u64,
    pub crc: String,
    /// `None`: carpid would flash it. Otherwise why not.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Status {
    pub rnse: Target,
    pub teensy: Target,
    pub protocol: Protocol,
    /// carpid is running a firmware job (maybe one another client started).
    pub busy: bool,
    /// Sector images uploaded through the portal.
    pub uploads: Vec<Upload>,
}

// carpid's types, field for field.

impl From<carpi_proto::FirmwareIdentity> for Identity {
    fn from(i: carpi_proto::FirmwareIdentity) -> Self {
        Self {
            version: i.version,
            build_id: i.build_id,
        }
    }
}

impl From<carpi_proto::FirmwareInstalled> for Installed {
    fn from(i: carpi_proto::FirmwareInstalled) -> Self {
        use carpi_proto::FirmwareInstalled as I;
        match i {
            I::Unknown => Self::Unknown,
            I::Legacy => Self::Legacy,
            I::Reported(id) => Self::Reported(id.into()),
        }
    }
}

impl From<carpi_proto::FirmwareComparison> for Comparison {
    fn from(c: carpi_proto::FirmwareComparison) -> Self {
        use carpi_proto::FirmwareComparison as C;
        match c {
            C::Unknown => Self::Unknown,
            C::Newer => Self::Newer,
            C::Same => Self::Same,
            C::Older => Self::Older,
            C::Incompatible => Self::Incompatible,
        }
    }
}

impl From<carpi_proto::FirmwareTarget> for Target {
    fn from(t: carpi_proto::FirmwareTarget) -> Self {
        Self {
            installed: t.installed.into(),
            bundled: t.bundled.map(Into::into),
            comparison: t.comparison.into(),
            detail: t.detail,
            blocked: t.blocked,
        }
    }
}

impl From<carpi_proto::FirmwareProtocol> for Protocol {
    fn from(p: carpi_proto::FirmwareProtocol) -> Self {
        use carpi_proto::FirmwareProtocol as P;
        match p {
            P::Unknown => Self::Unknown,
            P::Legacy => Self::Legacy,
            P::Negotiated(v) => Self::Negotiated(v),
        }
    }
}

impl From<carpi_proto::SectorUpload> for Upload {
    fn from(u: carpi_proto::SectorUpload) -> Self {
        Self {
            name: u.name,
            sector: u.sector,
            size: u.size,
            crc: u.crc,
            problem: u.problem,
        }
    }
}

impl From<carpi_proto::FirmwareStatus> for Status {
    fn from(s: carpi_proto::FirmwareStatus) -> Self {
        Self {
            rnse: s.rnse.into(),
            teensy: s.teensy.into(),
            protocol: s.protocol.into(),
            busy: s.busy,
            uploads: s.uploads.into_iter().map(Into::into).collect(),
        }
    }
}

/// Which firmware an install writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Rnse,
    Teensy,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rnse => "RNS-E",
            Self::Teensy => "Teensy",
        }
    }

    /// The request after the second push.
    pub fn install(self) -> carpi_proto::FirmwareAction {
        match self {
            Self::Rnse => carpi_proto::FirmwareAction::InstallRnse { confirm: true },
            Self::Teensy => carpi_proto::FirmwareAction::InstallTeensy { confirm: true },
        }
    }

    /// What the user must know before the second push.
    fn warnings(self) -> &'static [(&'static str, &'static str, bool)] {
        match self {
            Self::Rnse => &[
                ("Ignition on, engine off", "Keep power on", true),
                ("RNS-E screen goes dark", "Restarts when done", false),
                ("Takes several minutes", "", false),
            ],
            Self::Teensy => &[("Video stops briefly", "", false)],
        }
    }
}

/// What a firmware job did, in words.
fn job_title(action: Option<&carpi_proto::FirmwareAction>) -> &'static str {
    use carpi_proto::FirmwareAction as A;
    match action {
        Some(A::InstallRnse { .. }) => "RNS-E install",
        Some(A::InstallTeensy { .. }) => "Teensy install",
        Some(A::FlashSector { .. }) => "Flash Sector",
        _ => "Version check",
    }
}

/// Characters per line that fit a full-width row label at 800x480.
const WRAP: usize = 64;

/// `text` in lines of at most `width` characters, broken at spaces where it can be.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let cut: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            lines.push(cut);
        }
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn capital(s: &str) -> String {
    match s.get(..1) {
        Some(f) => f.to_uppercase() + &s[1..],
        None => String::new(),
    }
}

/// The screen. `armed` is the install waiting for its second push.
pub fn view(status: &Status, job: &FirmwareJob, armed: Option<Unit>) -> View {
    view_dev(status, job, armed, None)
}

/// The screen, with `sector` the Flash Sector upload armed for its second push.
pub fn view_dev(
    status: &Status,
    job: &FirmwareJob,
    armed: Option<Unit>,
    sector: Option<&str>,
) -> View {
    let mut v = View::new(Layout::List, "SDLink firmware", true);
    if let Some(note) = &job.note {
        v = v.note(note.clone());
    }
    let info = |label: &str, value: String| Row::new(RowKind::Info, label, value);
    let running = job.job.running();
    let busy = status.busy || running;
    // The job first, where it's seen without scrolling: a headline with the percent, then
    // the message over as many full-width lines as it needs (a value would be cut off).
    let title = job_title(job.action.as_ref());
    if running {
        let pct = match job.job.progress {
            Some((done, Some(total))) if total > 0 => {
                format!(" · {} %  ({done} of {total})", done * 100 / total)
            }
            _ => String::new(),
        };
        let pct = if pct.is_empty() {
            " · running".to_string()
        } else {
            pct
        };
        v.row(info(&format!("{title}{pct}"), String::new()));
        for line in wrap(&capital(&job.job.message), WRAP) {
            v.row(info(&line, String::new()).dimmed(true));
        }
        if let Some(unit) = job.installing() {
            for (label, value, alert) in unit.warnings() {
                v.row(info(label, (*value).into()).alert(*alert));
            }
        }
    } else if let Some((ok, m)) = &job.job.finished {
        v.row(
            info(
                &format!("{title}: {}", if *ok { "Done" } else { "Failed" }),
                String::new(),
            )
            .alert(!ok),
        );
        for line in wrap(&capital(m), WRAP) {
            v.row(info(&line, String::new()).alert(!ok));
        }
    }
    v.row(info("SDLink protocol", status.protocol.label()));
    v.row(
        Row::new(
            RowKind::Button,
            "Check versions",
            if busy { "Busy" } else { "" },
        )
        .button("Check")
        .act(if busy { Act::None } else { Act::FirmwareProbe })
        .dimmed(busy),
    );
    let mut start = -1;
    for (unit, target) in [(Unit::Rnse, &status.rnse), (Unit::Teensy, &status.teensy)] {
        let name = unit.name();
        v.section(name);
        v.row(info("Installed", target.installed.label()));
        if let Some(detail) = target.detail.as_deref().filter(|d| !d.trim().is_empty()) {
            for line in wrap(detail, WRAP) {
                v.row(info(&line, String::new()).dimmed(true));
            }
        }
        v.row(info(
            "Bundled",
            target
                .bundled
                .as_ref()
                .map_or_else(|| "Unavailable".into(), Identity::label),
        ));
        v.row(info("Status", target.update_label().into()));
        if let Some(why) = &target.blocked {
            for line in wrap(&format!("Can't install: {why}"), WRAP) {
                v.row(info(&line, String::new()).alert(true));
            }
        }
        if !target.installable(busy) {
            continue;
        }
        let asking = armed == Some(unit);
        if asking {
            for (label, value, alert) in unit.warnings() {
                v.row(info(label, (*value).into()).alert(*alert));
            }
        }
        let update = target.comparison == Comparison::Newer;
        let label = match (asking, update) {
            (false, true) => "Install update",
            (false, false) => "Replace with bundled",
            (true, true) => "Confirm install",
            (true, false) => "Confirm replace",
        };
        let value = if update {
            format!(
                "{name} → {}",
                target
                    .bundled
                    .as_ref()
                    .map_or_else(String::new, Identity::label)
            )
        } else {
            String::new()
        };
        let at = v.row(
            Row::new(RowKind::Button, label, value)
                .button(if update { "Install" } else { "Replace" })
                .act(Act::FirmwareInstall(unit))
                .alert(asking),
        );
        if asking {
            start = at;
        }
    }
    if let Some(at) = flash_sector(&mut v, &status.uploads, busy, sector) {
        start = at;
    }
    v.start = start;
    v.finish()
}

/// Advanced: Flash Sector. Uploaded `[prefix]sector_<N>.bin` files; shown only when there
/// are some. Returns the armed button's focus.
fn flash_sector(v: &mut View, uploads: &[Upload], busy: bool, armed: Option<&str>) -> Option<i32> {
    if uploads.is_empty() {
        return None;
    }
    v.section("Advanced: Flash Sector");
    v.row(Row::new(RowKind::Info, "Raw Write", "No content checks").alert(true));
    let mut start = None;
    for u in uploads {
        let asking = armed == Some(u.name.as_str());
        if let Some(why) = &u.problem {
            v.row(Row::new(RowKind::Info, &u.name, why.clone()).alert(true));
            continue;
        }
        if asking {
            for (label, value, alert) in Unit::Rnse.warnings() {
                v.row(Row::new(RowKind::Info, *label, *value).alert(*alert));
            }
        }
        let label = if asking {
            format!("Confirm flash sector {}", u.sector)
        } else {
            u.name.clone()
        };
        let at = v.row(
            Row::new(
                RowKind::Button,
                &label,
                format!("Sector {} · CRC {}", u.sector, u.crc),
            )
            .button("Flash")
            .act(if busy {
                Act::None
            } else {
                Act::FlashSector(u.name.clone())
            })
            .dimmed(busy)
            .alert(asking),
        );
        if asking {
            start = Some(at);
        }
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::UpdateJob;

    fn identity(version: &str) -> Identity {
        Identity {
            version: version.into(),
            build_id: None,
        }
    }

    fn target(comparison: Comparison) -> Target {
        Target {
            installed: Installed::Reported(identity("9")),
            bundled: Some(identity("10")),
            comparison,
            ..Target::default()
        }
    }

    fn installs(v: &View) -> Vec<&Row> {
        v.rows
            .iter()
            .filter(|r| matches!(r.act, Act::FirmwareInstall(_)))
            .collect()
    }

    #[test]
    fn comparison_comes_from_backend_and_requires_both_identities() {
        let mut target = target(Comparison::Unknown);
        assert_eq!(target.update_label(), "Unknown");
        target.comparison = Comparison::Newer;
        assert_eq!(target.update_label(), "Update available");
        // Lexicographical and numerical guesses must not override backend evidence.
        target.comparison = Comparison::Older;
        assert_eq!(target.update_label(), "Newer than bundle");
        target.installed = Installed::Legacy;
        target.comparison = Comparison::Newer;
        assert_eq!(target.update_label(), "Update available");
        target.installed = Installed::Unknown;
        assert_eq!(target.update_label(), "Installed unknown");
        target.bundled = None;
        assert_eq!(target.update_label(), "No bundle");
    }

    #[test]
    fn install_buttons_follow_bundle_block_busy_and_comparison() {
        for (comparison, shown) in [
            (Comparison::Newer, true),
            (Comparison::Unknown, true),
            (Comparison::Older, true),
            (Comparison::Same, false),
            (Comparison::Incompatible, false),
        ] {
            assert_eq!(
                target(comparison).installable(false),
                shown,
                "{comparison:?}"
            );
        }
        assert!(!target(Comparison::Newer).installable(true), "busy");
        let mut t = target(Comparison::Newer);
        t.blocked = Some("Ignition is off".into());
        assert!(!t.installable(false));
        t.blocked = None;
        t.bundled = None;
        assert!(!t.installable(false));

        let status = Status {
            rnse: target(Comparison::Newer),
            teensy: target(Comparison::Unknown),
            ..Status::default()
        };
        let v = view(&status, &FirmwareJob::default(), None);
        let rows = installs(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "Install update");
        assert_eq!(rows[0].act, Act::FirmwareInstall(Unit::Rnse));
        assert_eq!(rows[1].label, "Replace with bundled");
        assert!(rows[1].value.is_empty());
        assert_eq!(rows[1].act, Act::FirmwareInstall(Unit::Teensy));

        // carpid busy (another client's job), or our own job running: no installs, no probe.
        let busy = Status {
            busy: true,
            ..status.clone()
        };
        let v = view(&busy, &FirmwareJob::default(), None);
        assert!(installs(&v).is_empty());
        assert!(!v.rows.iter().any(|r| r.act == Act::FirmwareProbe));
        let job = FirmwareJob {
            action: Some(carpi_proto::FirmwareAction::Probe),
            job: UpdateJob {
                job: Some(3),
                ..UpdateJob::default()
            },
            ..FirmwareJob::default()
        };
        let v = view(&status, &job, None);
        assert!(installs(&v).is_empty());
        let probe = v.rows.iter().find(|r| r.label == "Check versions").unwrap();
        assert_eq!(probe.act, Act::None);
        assert!(probe.dimmed);
    }

    #[test]
    fn blocked_reason_and_detail_are_shown() {
        let status = Status {
            rnse: Target {
                detail: Some("Table 3, formats 1".into()),
                blocked: Some("Ignition is off".into()),
                ..target(Comparison::Newer)
            },
            ..Status::default()
        };
        let v = view(&status, &FirmwareJob::default(), None);
        assert!(v.rows.iter().any(|r| r.label == "Table 3, formats 1"));
        let blocked = v
            .rows
            .iter()
            .find(|r| r.label == "Can't install: Ignition is off")
            .unwrap();
        assert!(blocked.alert);
        assert!(installs(&v).is_empty());
    }

    #[test]
    fn armed_install_shows_its_warnings_and_asks_again() {
        let status = Status {
            rnse: target(Comparison::Newer),
            teensy: target(Comparison::Older),
            ..Status::default()
        };
        let v = view(&status, &FirmwareJob::default(), None);
        assert!(!v.rows.iter().any(|r| r.label == "Ignition on, engine off"));
        let v = view(&status, &FirmwareJob::default(), Some(Unit::Rnse));
        for label in [
            "Ignition on, engine off",
            "RNS-E screen goes dark",
            "Takes several minutes",
        ] {
            assert!(v.rows.iter().any(|r| r.label == label), "{label}");
        }
        assert!(v.rows.iter().any(|r| r.value.contains("Keep power on")));
        assert!(!v.rows.iter().any(|r| r.label == "Video stops briefly"));
        let rows = installs(&v);
        assert_eq!(rows[0].label, "Confirm install");
        assert!(rows[0].alert);
        assert_eq!(rows[1].label, "Replace with bundled");
        assert_eq!(v.start, rows[0].focus_at, "focus stays on the armed button");
        let v = view(&status, &FirmwareJob::default(), Some(Unit::Teensy));
        assert!(v.rows.iter().any(|r| r.label == "Video stops briefly"));
        assert_eq!(installs(&v)[1].label, "Confirm replace");
    }

    #[test]
    fn job_progress_and_result_are_shown() {
        let mut job = FirmwareJob {
            action: Some(Unit::Rnse.install()),
            job: UpdateJob {
                job: Some(4),
                stage: "stage".into(),
                message: "writing sector 107".into(),
                progress: Some((25, Some(100))),
                ..UpdateJob::default()
            },
            ..FirmwareJob::default()
        };
        let v = view(&Status::default(), &job, None);
        // The headline leads with the percent; it comes before the protocol row.
        let at = |l: &str| v.rows.iter().position(|r| r.label == l).unwrap();
        assert!(at("RNS-E install · 25 %  (25 of 100)") < at("SDLink protocol"));
        assert!(v.rows.iter().any(|r| r.label == "Writing sector 107"));
        assert!(v.rows.iter().any(|r| r.label == "Ignition on, engine off"));
        job.job.finished = Some((false, "RNS-E did not reply".into()));
        let v = view(&Status::default(), &job, None);
        let row = v
            .rows
            .iter()
            .find(|r| r.label == "RNS-E install: Failed")
            .unwrap();
        assert!(row.alert);
        assert!(
            v.rows
                .iter()
                .any(|r| r.label == "RNS-E did not reply" && r.alert)
        );
        // A long reason is all there, over several lines.
        let long = "the RNS-E didn't answer a read at 806ACC00: no complete reply; is the RNS-E awake? Check it, then run Check versions";
        job.job.finished = Some((false, long.into()));
        let v = view(&Status::default(), &job, None);
        let text: Vec<&str> = v
            .rows
            .iter()
            .filter(|r| r.alert)
            .map(|r| r.label.as_str())
            .skip(1)
            .collect();
        assert!(text.len() > 1 && text.iter().all(|l| l.chars().count() <= WRAP));
        assert_eq!(text.join(" "), capital(long));
    }

    fn upload(name: &str, sector: u16, problem: Option<&str>) -> Upload {
        Upload {
            name: name.into(),
            sector,
            size: 0x20000,
            crc: "1A2B3C4D".into(),
            problem: problem.map(Into::into),
        }
    }

    #[test]
    fn flash_sector_lists_uploads_and_arms_one() {
        let v = view(&Status::default(), &FirmwareJob::default(), None);
        assert!(!v.rows.iter().any(|r| matches!(r.act, Act::FlashSector(_))));
        let status = Status {
            uploads: vec![
                upload("sector_7.bin", 7, Some("outside 32-511")),
                upload("sector_96.bin", 96, None),
            ],
            ..Status::default()
        };
        let v = view(&status, &FirmwareJob::default(), None);
        let bad = v.rows.iter().find(|r| r.label == "sector_7.bin").unwrap();
        assert!(bad.alert && bad.act == Act::None);
        let ok = v.rows.iter().find(|r| r.label == "sector_96.bin").unwrap();
        assert_eq!(ok.act, Act::FlashSector("sector_96.bin".into()));
        assert_eq!(ok.value, "Sector 96 · CRC 1A2B3C4D");
        assert!(!v.rows.iter().any(|r| r.label == "Ignition on, engine off"));

        let v = view_dev(
            &status,
            &FirmwareJob::default(),
            None,
            Some("sector_96.bin"),
        );
        let armed = v
            .rows
            .iter()
            .find(|r| r.label == "Confirm flash sector 96")
            .unwrap();
        assert!(armed.alert);
        assert_eq!(v.start, armed.focus_at);
        assert!(v.rows.iter().any(|r| r.label == "Ignition on, engine off"));

        let busy = Status {
            busy: true,
            ..status
        };
        let v = view(&busy, &FirmwareJob::default(), None);
        let row = v.rows.iter().find(|r| r.label == "sector_96.bin").unwrap();
        assert_eq!(row.act, Act::None);
    }

    #[test]
    fn sector_flash_job_is_titled_and_warns() {
        let job = FirmwareJob {
            action: Some(carpi_proto::FirmwareAction::FlashSector {
                name: "sector_96.bin".into(),
                confirm: true,
            }),
            job: UpdateJob {
                job: Some(5),
                message: "writing sector 96".into(),
                ..UpdateJob::default()
            },
            ..FirmwareJob::default()
        };
        let v = view(&Status::default(), &job, None);
        assert!(v.rows.iter().any(|r| r.label == "Flash Sector · running"));
        assert!(v.rows.iter().any(|r| r.label == "Writing sector 96"));
        assert!(v.rows.iter().any(|r| r.label == "Ignition on, engine off"));
    }

    #[test]
    fn empty_status_does_not_claim_legacy_or_an_update() {
        let v = view(&Status::default(), &FirmwareJob::default(), None);
        assert_eq!(v.rows[0].value, "Unavailable");
        let installed = v.rows.iter().find(|r| r.label == "Installed").unwrap();
        assert_eq!(installed.value, "Unknown");
        let status = v.rows.iter().find(|r| r.label == "Status").unwrap();
        assert_eq!(status.value, "No bundle");
        assert!(installs(&v).is_empty());
        assert_eq!(Protocol::Legacy.label(), "Legacy protocol");
        assert_eq!(Protocol::Negotiated(String::new()).label(), "Unavailable");
    }

    #[test]
    fn proto_status_converts_field_for_field() {
        let proto = carpi_proto::FirmwareStatus {
            rnse: carpi_proto::FirmwareTarget {
                installed: carpi_proto::FirmwareInstalled::Legacy,
                bundled: Some(carpi_proto::FirmwareIdentity {
                    version: "v2".into(),
                    build_id: Some("abc".into()),
                }),
                comparison: carpi_proto::FirmwareComparison::Newer,
                detail: Some("table 3".into()),
                blocked: None,
            },
            teensy: carpi_proto::FirmwareTarget {
                blocked: Some("chain running".into()),
                ..Default::default()
            },
            protocol: carpi_proto::FirmwareProtocol::Negotiated("1".into()),
            busy: true,
            uploads: vec![carpi_proto::SectorUpload {
                name: "sector_96.bin".into(),
                sector: 96,
                ..Default::default()
            }],
        };
        let json = serde_json::to_string(&proto).unwrap();
        let ours: Status = proto.into();
        // The fixtures' shape is carpid's wire shape.
        assert_eq!(serde_json::from_str::<Status>(&json).unwrap(), ours);
        assert_eq!(ours.rnse.installed, Installed::Legacy);
        assert_eq!(ours.rnse.comparison, Comparison::Newer);
        assert_eq!(ours.rnse.detail.as_deref(), Some("table 3"));
        assert_eq!(ours.teensy.blocked.as_deref(), Some("chain running"));
        assert_eq!(ours.protocol, Protocol::Negotiated("1".into()));
        assert!(ours.busy);
        assert_eq!(ours.uploads[0].sector, 96);
    }

    #[test]
    fn prepared_fixtures_cover_newer_and_legacy_without_a_live_backend() {
        let newer: crate::client::Fixture =
            serde_json::from_str(include_str!("../fixtures/firmware-newer.json")).unwrap();
        assert_eq!(newer.firmware.protocol.label(), "Negotiated 1");
        assert_eq!(newer.firmware.rnse.comparison, Comparison::Newer);
        assert_eq!(newer.firmware.teensy.comparison, Comparison::Same);
        assert_eq!(newer.firmware.rnse.update_label(), "Update available");
        assert!(newer.firmware.rnse.detail.is_some());
        let v = view(&newer.firmware, &FirmwareJob::default(), None);
        assert_eq!(installs(&v).len(), 1, "Same offers no install");
        let legacy: crate::client::Fixture =
            serde_json::from_str(include_str!("../fixtures/firmware-legacy.json")).unwrap();
        assert_eq!(legacy.firmware.protocol, Protocol::Legacy);
        assert_eq!(legacy.firmware.rnse.installed, Installed::Legacy);
        assert_eq!(legacy.firmware.rnse.update_label(), "Unknown");
        assert!(legacy.firmware.teensy.blocked.is_some());
        let v = view(&legacy.firmware, &FirmwareJob::default(), None);
        let rows = installs(&v);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Replace with bundled");
    }
}
