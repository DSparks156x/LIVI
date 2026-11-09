// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! Every screen of the Car Integration tab at 800x480, drawn offscreen with LIVI's own
//! SettingsView (and our page over it for the plot and the picker), night and day:
//!
//! ```text
//! cargo run -p livi-carpi-shots -- <dir>
//! ```

use std::cell::Cell as StdCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use carpi_proto::{AvsdRange, DisCellSetting, DisPageSetting, Reading, Settings, Value};
use livi_carpi_ui::client::{Client, Fixture};
use livi_carpi_ui::config::Saved;
use livi_carpi_ui::view::{CarScreen as S, View};
use livi_carpi_ui::{PageId, Pages};
use livi_ui_model::carpi::{native, overlay};
use livi_ui_model::settings::view::{Row, RowKind as M, Screen};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Image, ModelRc, PhysicalSize, SharedString, VecModel};

slint::slint! {
    import "../../livi-ui/ui/fonts/Roboto-400.ttf";
    import "../../livi-ui/ui/fonts/Roboto-500.ttf";
    import "../../livi-ui/ui/fonts/Roboto-700.ttf";
    import { Theme, Ui } from "../../livi-ui/ui/theme.slint";
    import { Focus, FocusZone } from "../../livi-ui/ui/focus.slint";
    import { SettingsView, SettingsState, RowData, RowKind, ScreenKind, PickData, KnobData } from "../../livi-ui/ui/settings/view.slint";
    import { ToolData } from "../../livi-ui/ui/settings/row.slint";
    import { CarpiView, CarpiState, CarpiLayout, CarpiKind, CarpiPill, CarpiRow, CarpiCell, CarpiSpot, CarpiPoint, CarpiTick, CarpiKeyboard, CarpiKeyboardView, CarpiKey, CarpiKeyRow } from "../../livi-carpi-ui/ui/carpi.slint";
    export { Theme, Ui, Focus, FocusZone, SettingsState, RowData, RowKind, ScreenKind, PickData, KnobData, ToolData, CarpiState, CarpiLayout, CarpiKind, CarpiPill, CarpiRow, CarpiCell, CarpiSpot, CarpiPoint, CarpiTick, CarpiKeyboard, CarpiKey, CarpiKeyRow }

    export component Shot inherits Window {
        width: 800px;
        height: 480px;
        default-font-family: "Roboto";
        background: Theme.background;
        in property <bool> over;
        // Where LIVI's tab bar sits.
        Rectangle {
            x: 0;
            width: 64px;
            background: Theme.background.mix(Theme.dark ? #ffffff : #000000, 0.88);
        }
        SettingsView {
            x: 64px;
            width: 736px;
            height: 480px;
        }
        if root.over: CarpiView {
            x: 64px;
            width: 736px;
            height: 480px;
        }
        if CarpiKeyboard.shown: CarpiKeyboardView {
            x: 64px;
            width: 736px;
            height: 480px;
        }
    }
}

include!("../../livi-carpi-ui/ui/glue.rs");

thread_local! {
    static NOW: StdCell<Duration> = const { StdCell::new(Duration::ZERO) };
}

struct Offscreen(Rc<MinimalSoftwareWindow>);

impl Platform for Offscreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
    fn duration_since_start(&self) -> Duration {
        NOW.get()
    }
}

fn icon(name: &str) -> Option<Image> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../livi-ui/ui/icons")
        .join(format!("{name}.svg"));
    Image::load_from_path(&path).ok()
}

/// LIVI's own icon names for its settings keys (`carName` and so on).
fn row_icon(r: &Row) -> Option<Image> {
    let name = r.icon?;
    icon(livi_ui_model::icons::settings_icon(name).unwrap_or(name))
}

fn kind(k: M) -> RowKind {
    match k {
        M::Route => RowKind::Route,
        M::Value => RowKind::Value,
        M::Switch => RowKind::Toggle,
        M::Slider => RowKind::Slider,
        M::KeyBinding => RowKind::KeyBinding,
        M::Custom => RowKind::Custom,
        M::Devices => RowKind::Devices,
        M::Info => RowKind::Info,
        M::Button => RowKind::Button,
        M::Progress => RowKind::Progress,
        M::Item => RowKind::Item,
        M::Device => RowKind::Device,
        M::Picture => RowKind::Picture,
        M::Pick => RowKind::Pick,
    }
}

/// livi-ui's `row_data`, for the rows the tab makes.
fn row_data(r: &Row, focus_at: i32) -> RowData {
    let image = row_icon(r);
    RowData {
        kind: kind(r.kind),
        label: r.label.as_str().into(),
        has_icon: image.is_some(),
        icon: image.unwrap_or_default(),
        value: r.value.as_str().into(),
        checked: r.checked,
        level: r.level,
        enabled: r.enabled,
        node: r.node as i32,
        focus_at,
        button: r.button.as_str().into(),
        dimmed: r.dimmed,
        mono: r.mono,
        alert: r.alert,
        tool_offset: -1,
        tools: ModelRc::from(Rc::new(VecModel::<ToolData>::default())),
        ..Default::default()
    }
}

struct Shots {
    window: Rc<MinimalSoftwareWindow>,
    ui: Shot,
    models: Models,
    dir: std::path::PathBuf,
    n: StdCell<u32>,
}

impl Shots {
    /// Draws `screen` (a LIVI screen) with focus on `slot`.
    fn settings(&self, screen: &Screen, slot: Option<usize>) {
        let s = self.ui.global::<SettingsState>();
        s.set_can_back(true);
        s.set_show_apply(false);
        match screen {
            Screen::List { title, rows, .. } => {
                s.set_title(title.as_str().into());
                let mut focus = Vec::new();
                let data: Vec<RowData> = rows
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let at = if r.focusable() {
                            focus.push(i as i32);
                            focus.len() as i32 - 1
                        } else {
                            -1
                        };
                        row_data(r, at)
                    })
                    .collect();
                s.set_focus_rows(ModelRc::from(Rc::new(VecModel::from(focus))));
                s.set_rows(ModelRc::from(Rc::new(VecModel::from(data))));
                s.set_kind(ScreenKind::List);
            }
            Screen::Select { title, options, .. } => {
                s.set_title(title.as_str().into());
                s.set_picks(ModelRc::from(Rc::new(VecModel::from(
                    options
                        .iter()
                        .map(|p| PickData {
                            label: p.label.as_str().into(),
                            selected: p.selected,
                        })
                        .collect::<Vec<_>>(),
                ))));
                s.set_kind(ScreenKind::Select);
            }
            _ => {}
        }
        self.focus(slot.map_or(-1, |x| x as i32 + 1));
    }

    fn focus(&self, at: i32) {
        let f = self.ui.global::<Focus>();
        f.set_zone(if at >= 0 {
            FocusZone::Main
        } else {
            FocusZone::None
        });
        // Twice: the list's height is only known once it's drawn.
        for _ in 0..2 {
            f.set_main_index(-1);
            self.settle();
            f.set_main_index(at);
            self.settle();
        }
    }

    /// Draws one of our views the way livi-ui does: LIVI's rows, or our page over them.
    fn view(&self, v: &View, focus: Option<i32>, dark: bool, name: &str) {
        self.ui.global::<Theme>().set_dark(dark);
        if overlay(v) {
            self.ui.set_over(true);
            self.models.fill(&self.ui.global::<CarpiState>(), v);
            self.focus(focus.unwrap_or(v.start));
        } else {
            self.ui.set_over(false);
            let n = native(v);
            let slot = match focus {
                Some(at) => n.slot_of(v, at),
                None => n.slot_of(v, v.start),
            };
            self.settings(&n.screen, slot);
        }
        self.save(name);
    }

    /// Runs animations to their end.
    fn settle(&self) {
        for _ in 0..20 {
            NOW.set(NOW.get() + Duration::from_millis(100));
            slint::platform::update_timers_and_animations();
            self.window.request_redraw();
            self.window.draw_if_needed(|_| {});
        }
    }

    fn save(&self, name: &str) {
        let (w, h) = (800usize, 480usize);
        let mut buf = vec![Rgb565Pixel(0); w * h];
        self.window.request_redraw();
        self.window.draw_if_needed(|r| {
            r.render(&mut buf, w);
        });
        let mut img = image::RgbImage::new(w as u32, h as u32);
        for (i, p) in buf.iter().enumerate() {
            let (r, g, b) = (p.0 >> 11, (p.0 >> 5) & 0x3f, p.0 & 0x1f);
            img.put_pixel(
                (i % w) as u32,
                (i / w) as u32,
                image::Rgb([
                    (r * 255 / 31) as u8,
                    (g * 255 / 63) as u8,
                    (b * 255 / 31) as u8,
                ]),
            );
        }
        self.n.set(self.n.get() + 1);
        let path = self.dir.join(format!("{:02}-{name}.png", self.n.get()));
        img.save(&path).expect("png");
        println!("{}", path.display());
    }
}

/// The fixture's settings with every section filled, as carpid sends them.
fn settings(mut s: Settings) -> Settings {
    let cell = |key: &str, decimals: u8, unit: Option<&str>| DisCellSetting {
        key: key.into(),
        decimals,
        scale: 1.0,
        unit: unit.map(str::to_string),
        icon: None,
    };
    s.dis.pages = vec![
        DisPageSetting {
            name: "Engine".into(),
            cells: vec![
                cell("engine.coolant_c", 0, None),
                cell("engine.oil_temp_c", 0, Some("°F")),
                cell("power.battery_v", 1, None),
                cell("engine.boost.actual", 2, Some("bar")),
                cell("ambient.outside_temp_c", 1, None),
            ],
        },
        DisPageSetting {
            name: "Haldex".into(),
            cells: vec![
                cell("awd.oil_temp_c", 0, None),
                cell("awd.plate_temp_c", 0, None),
            ],
        },
    ];
    s.clock.timezone = "America/Denver".into();
    s.clock.timezones = [
        "Africa/Cairo",
        "America/Chicago",
        "America/Denver",
        "America/Los_Angeles",
        "America/New_York",
        "America/Phoenix",
        "Asia/Tokyo",
        "Australia/Sydney",
        "Europe/Berlin",
        "Europe/London",
        "UTC",
    ]
    .map(String::from)
    .to_vec();
    s.portal.address = Some("http://10.10.0.1:5003".into());
    s.update.version = "carpid 0.1.0, release 2026.11.08 (stable)".into();
    s.power.shutdown = true;
    s.bindings_custom = true;
    s.dis.accel = ["0-60mph", "50-100kmh", "1/4mi", "speed"]
        .map(String::from)
        .to_vec();
    s.avsd.path = "/home/pi/avsd/avsd.env".into();
    s.avsd.found = true;
    let r = |key: &str, label: &str, unit: &str, min, max, step, default, zero_off| AvsdRange {
        key: key.into(),
        label: label.into(),
        unit: unit.into(),
        min,
        max,
        step,
        default,
        zero_off,
        help: String::new(),
    };
    s.avsd.ranges = vec![
        r("GAIN", "Gain", "dB", -12.0, 12.0, 1.0, 0.0, false),
        r(
            "LINKCAP", "Link cap", "kB/s", 200.0, 2000.0, 50.0, 900.0, false,
        ),
        r(
            "MAXVID",
            "Video budget",
            "kB/s",
            100.0,
            1800.0,
            50.0,
            700.0,
            false,
        ),
        r("CYC", "Frame cycle", "ms", 20.0, 200.0, 5.0, 40.0, false),
        r(
            "KEYINT",
            "Key frame every",
            "frames",
            10.0,
            300.0,
            10.0,
            60.0,
            true,
        ),
        r(
            "ENCTHREADS",
            "Encoder threads",
            "",
            1.0,
            4.0,
            1.0,
            2.0,
            false,
        ),
        r("DITHER", "Dither", "", 0.0, 1.0, 1.0, 1.0, false),
        r("TONEPAL", "Palette snap", "", 0.0, 1.0, 1.0, 1.0, false),
        r("NURELAX", "Relax", "", 0.0, 1.0, 1.0, 1.0, false),
    ];
    s.avsd.values.insert("GAIN".into(), 3.0);
    s
}

fn client() -> Client {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../livi-carpi-ui/fixtures/car.json");
    let mut f = Fixture::read(&path).expect("fixture");
    f.settings = f.settings.map(settings);
    let client = f.start();
    let extra = [
        ("engine.oil_temp_c", Value::Num(96.0)),
        ("engine.boost.actual", Value::Num(1840.0)),
        ("awd.oil_temp_c", Value::Num(54.0)),
        ("dis.state", Value::Text("ready".into())),
        ("dis.shown", Value::Text("nav".into())),
        ("projection.wireless", Value::Bool(true)),
        ("avsd.state", Value::Text("Streaming".into())),
        ("avsd.teensy", Value::Bool(true)),
        ("avsd.fps", Value::Num(29.8)),
        ("avsd.link_kbs", Value::Num(717.0)),
        ("avsd.cap_kbs", Value::Num(4500.0)),
        ("avsd.dropped", Value::Num(2.0)),
        ("avsd.stalls", Value::Num(0.0)),
    ];
    client.snapshot().datalog = Some(datalog());
    {
        use carpi_proto::{WifiNetwork, WifiSeen};
        let mut snap = client.snapshot();
        let net = |ssid: &str, secured, last: Option<u64>| WifiNetwork {
            ssid: ssid.into(),
            secured,
            hidden: false,
            last_used: last,
        };
        snap.wifi.networks = Some(vec![
            net("Stadium Walk-UF104", true, Some(1_762_600_000)),
            net("Pixel 10 Pro", true, Some(1_762_500_000)),
            net("Library Guest", false, None),
        ]);
        let seen = |ssid: &str, signal, secured, saved| WifiSeen {
            ssid: ssid.into(),
            signal,
            secured,
            saved,
        };
        snap.wifi.seen = Some(vec![
            seen("Stadium Walk-UF104", 82, true, true),
            seen("Stadium Walk-UF202", 64, true, false),
            seen("Social at Stadium Walk", 51, false, false),
            seen("TurtleTown", 30, true, false),
        ]);
        snap.wifi.job.finished = Some((true, "Stadium Walk-UF104 works, with the internet".into()));
    }
    client.snapshot().values.take(
        extra
            .iter()
            .map(|(n, v)| Reading {
                name: n.to_string(),
                value: Some(v.clone()),
                age_ms: 0,
            })
            .collect(),
        Instant::now(),
    );
    client
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map_or_else(|| std::path::PathBuf::from("shots"), Into::into);
    std::fs::create_dir_all(&dir).expect("dir");
    let only: Vec<String> = std::env::args().skip(2).collect();
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(window.clone()))).expect("platform");
    let ui = Shot::new().expect("ui");
    window.set_size(PhysicalSize::new(800, 480));
    ui.global::<Ui>().set_width(800.0);
    ui.global::<Ui>().set_height(480.0);
    ui.show().expect("show");
    let shots = Shots {
        window,
        ui,
        models: Models::default(),
        dir,
        n: StdCell::new(0),
    };
    shots.models.wire(&shots.ui.global::<CarpiState>());
    let _ = SharedString::new();

    let client = client();
    std::thread::sleep(Duration::from_millis(50));
    let mut pages = Pages::new(Saved::default(), None);
    let now = Instant::now();

    let tree: &[(&str, S)] = &[
        ("root", S::Root),
        ("status", S::Status),
        ("all-values", S::AllValues),
        ("buses", S::Buses),
        ("cluster", S::Cluster),
        ("readings-pages", S::ReadingPages),
        ("readings-page", S::ReadingPage(0)),
        ("cell", S::Cell(0, 1)),
        ("cell-value", S::CellValue(0, 1)),
        ("cell-icon", S::CellIcon(0, 1)),
        ("nav", S::Nav),
        ("calls", S::Calls),
        ("media", S::Media),
        ("accel", S::Accel),
        ("lighting", S::Lighting),
        ("curve", S::Curve),
        ("point", S::Point(1)),
        ("night", S::Night),
        ("controls", S::Controls),
        ("wheel", S::Wheel),
        ("faceplate", S::Faceplate),
        ("key", S::Key(3)),
        ("binding", S::Binding(3, 0)),
        ("logs", S::Logs),
        ("logging", S::Logging),
        ("power", S::Power),
        ("zones", S::Zones),
        ("zone", S::Zone(1)),
        ("avsd", S::Avsd),
        ("picture", S::Picture),
        ("portal", S::Portal),
        ("pin", S::Pin),
        ("updates", S::Updates),
        ("update-confirm", S::UpdateConfirm),
        ("wifi", S::Wifi),
        ("wifi-net", S::WifiNet(0)),
        ("wifi-add", S::WifiAdd),
        ("advanced", S::Advanced),
    ];
    for (name, screen) in tree {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        pages.goto(*screen);
        let v = pages.view(PageId::Car, &client.snapshot(), now);
        shots.view(&v, None, true, name);
        shots.view(&v, None, false, &format!("{name}-day"));
        // The last row too, on long screens.
        let last = v.targets() as i32 - 1;
        if v.rows.len() > 7 && last > v.start {
            shots.view(&v, Some(last), true, &format!("{name}-end"));
        }
    }
    // The on-screen keyboard over the Wi-Fi screen, both layouts.
    use livi_carpi_ui::keyboard::{Input, Keyboard, Prefs, Style};
    let boards = [
        (
            "keyboard-knob",
            Style::Knob,
            true,
            "Password for Stadium Walk-UF104",
        ),
        (
            "keyboard-touch",
            Style::Touch,
            true,
            "Password for Stadium Walk-UF104",
        ),
        ("keyboard-line", Style::Knob, false, "Accel test line 2"),
    ];
    for (name, style, secret, title) in boards {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        let prefs = Prefs {
            style,
            ..Prefs::default()
        };
        let initial = if secret { "tt8n" } else { "50-100kmh" };
        let mut k = Keyboard::open(
            title,
            initial,
            secret,
            if secret { "Join" } else { "Save" },
            prefs,
        );
        for _ in 0..13 {
            k.input(Input::Turn(1));
        }
        fill_keyboard(&shots.ui.global::<CarpiKeyboard>(), Some(&k));
        pages.goto(S::Wifi);
        let v = pages.view(PageId::Car, &client.snapshot(), now);
        shots.view(&v, None, true, name);
        shots.view(&v, None, false, &format!("{name}-day"));
        fill_keyboard(&shots.ui.global::<CarpiKeyboard>(), None);
    }
}

/// Two profiles and a run that ended, as carpid reports them.
fn datalog() -> carpi_proto::DatalogStatus {
    use carpi_proto::{DatalogLoad, DatalogProfile, DatalogProfileStatus, DatalogRun};
    let p = |name: &str, values: &[&str], rate_hz, load| DatalogProfileStatus {
        profile: DatalogProfile {
            name: name.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            rate_hz,
        },
        load,
    };
    carpi_proto::DatalogStatus {
        profiles: vec![
            p(
                "Boost",
                &[
                    "engine.rpm",
                    "engine.boost.actual",
                    "engine.boost.specified",
                    "engine.n75_duty_pct",
                    "engine.load_pct",
                    "engine.throttle_pct",
                    "engine.intake_temp_c",
                    "vehicle.speed_kmh",
                ],
                10,
                DatalogLoad {
                    bus_values: 2,
                    diag_values: 6,
                    diag_groups: 3,
                    diag_modules: 1,
                    diag_hz: Some(4.4),
                },
            ),
            p(
                "Haldex",
                &["awd.oil_temp_c", "awd.plate_temp_c", "engine.oil_temp_c"],
                2,
                DatalogLoad {
                    bus_values: 0,
                    diag_values: 3,
                    diag_groups: 2,
                    diag_modules: 2,
                    diag_hz: Some(2.0),
                },
            ),
        ],
        selected: Some("Boost".into()),
        running: None,
        last: Some(DatalogRun {
            label: "2026-11-08 #03".into(),
            date: "2026-11-08".into(),
            number: 3,
            profile: "Boost".into(),
            values: 8,
            rate_hz: 10,
            started: "2026-11-08T09:40:00".into(),
            elapsed_ms: 481_200,
            rows: 4812,
            dir: "/home/pi/logs/2026-11-08/03".into(),
            summary: "8 values, 10 Hz, 4 812 rows".into(),
            error: None,
        }),
    }
}
