// SPDX-FileCopyrightText: 2026 Drew Sparks
// SPDX-License-Identifier: GPL-3.0-or-later

//! The pages drawn offscreen at 800x480 with Slint's software renderer, from the fixture, with a
//! strip where LIVI's tab bar goes:
//!
//! ```text
//! cargo run -p livi-carpi-ui --features shots --example shots -- <dir>
//! ```

use std::cell::Cell as StdCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use carpi_proto::{Reading, ToastLevel, Value};
use livi_carpi_ui::client::Fixture;
use livi_carpi_ui::view::{Hit, View};
use livi_carpi_ui::{Client, PageId, Pages, config::Saved};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};

slint::slint! {
    import "../../livi-ui/ui/fonts/Roboto-400.ttf";
    import "../../livi-ui/ui/fonts/Roboto-500.ttf";
    import "../../livi-ui/ui/fonts/Roboto-700.ttf";
    import { Theme } from "../../livi-ui/ui/theme.slint";
    import { Focus, FocusZone } from "../../livi-ui/ui/focus.slint";
    import { CarpiView, CarpiState, CarpiLayout, CarpiKind, CarpiPill, CarpiRow, CarpiCell, CarpiSpot, CarpiPoint, CarpiTick, CarpiToast, CarpiToastView, CarpiKeyboard, CarpiKey, CarpiKeyRow, CarpiWidget, CarpiLine, CarpiItem, CarpiMark, CarpiEmpty } from "../ui/carpi.slint";
    export { Theme, Focus, FocusZone, CarpiState, CarpiLayout, CarpiKind, CarpiPill, CarpiRow, CarpiCell, CarpiSpot, CarpiPoint, CarpiTick, CarpiToast, CarpiKeyboard, CarpiKey, CarpiKeyRow, CarpiWidget, CarpiLine, CarpiItem, CarpiMark, CarpiEmpty }

    export component Shot inherits Window {
        width: 800px;
        height: 480px;
        default-font-family: "Roboto";
        background: Theme.background;
        // Where LIVI's tab bar sits.
        Rectangle {
            x: 0;
            width: 56px;
            background: Theme.background.mix(Theme.dark ? #ffffff : #000000, 0.88);
        }
        CarpiView {
            x: 56px;
            width: 744px;
            height: 480px;
        }
        CarpiToastView {
            window-width: 744px;
            x: 56px + (744px - self.width) / 2;
            y: 480px - self.height - 20px;
        }
    }
}

include!("../ui/glue.rs");

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

struct Shots {
    window: Rc<MinimalSoftwareWindow>,
    ui: Shot,
    models: Models,
    dir: std::path::PathBuf,
}

impl Shots {
    fn show(&self, view: &View, focus: i32, dark: bool) {
        self.ui.global::<Theme>().set_dark(dark);
        self.models.fill(&self.ui.global::<CarpiState>(), view);
        let f = self.ui.global::<Focus>();
        f.set_zone(if focus >= 0 {
            FocusZone::Main
        } else {
            FocusZone::None
        });
        f.set_main_index(-1);
        self.settle();
        f.set_main_index(focus);
        self.settle();
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
        let path = self.dir.join(format!("{name}.png"));
        img.save(&path).expect("png");
        println!("{}", path.display());
    }
}

/// The fixture, with some diag values as carpid would poll them.
fn client() -> Client {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/car.json");
    let client = Fixture::read(&path).expect("fixture").start();
    let diag = [
        ("engine.boost.actual", 1840.0),
        ("engine.boost.specified", 1900.0),
        ("engine.load_pct", 62.0),
        ("engine.throttle_pct", 38.0),
        ("engine.intake_temp_c", 31.0),
        ("engine.oil_temp_c", 96.0),
        ("engine.maf_gs", 48.5),
        ("engine.lambda.actual", 0.992),
    ];
    client.snapshot().values.take(
        diag.iter()
            .map(|(n, v)| Reading {
                name: n.to_string(),
                value: Some(Value::Num(*v)),
                age_ms: 0,
            })
            .collect(),
        Instant::now(),
    );
    client
}

fn row(v: &View, label: &str) -> Hit {
    Hit::Row(v.rows.iter().position(|r| r.label == label).expect(label))
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map_or_else(|| std::path::PathBuf::from("shots"), Into::into);
    std::fs::create_dir_all(&dir).expect("dir");
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Offscreen(window.clone()))).expect("platform");
    let ui = Shot::new().expect("ui");
    window.set_size(PhysicalSize::new(800, 480));
    ui.show().expect("show");
    let shots = Shots {
        window,
        ui,
        models: Models::default(),
        dir,
    };
    shots.models.wire(&shots.ui.global::<CarpiState>());

    let client = client();
    let mut pages = Pages::new(Saved::default(), None);
    let now = Instant::now();
    let view = |p: &Pages| p.view(PageId::Data, &client.snapshot(), now);

    // Panels, with a minute of history so graphs and peaks have something.
    pages.set_panels(demo());
    for i in 0..300u32 {
        let t = i as f64 / 10.0;
        let boost = 1013.0 + 900.0 * ((t * 0.35).sin().max(-0.4) + 0.1);
        let spec = 1013.0 + if (t * 0.35).sin() > 0.2 { 1050.0 } else { 80.0 };
        let vals = [
            ("engine.boost.actual", boost),
            ("engine.boost.specified", spec),
            ("engine.altitude_factor", 0.98),
            ("engine.rpm", 2400.0 + 1800.0 * (t * 0.35).sin().max(0.0)),
            ("engine.coolant_c", 91.0),
            ("engine.oil_temp_c", 108.0),
            ("awd.plate_temp_c", 48.0),
            ("power.battery_v", 14.2),
            ("ambient.outside_temp_c", 7.5),
            ("engine.n75_duty_pct", 71.0),
            ("engine.load_pct", 162.0),
            ("engine.timing_retard.cyl1_deg", -1.5),
            ("engine.timing_retard.cyl2_deg", -4.5),
            ("engine.timing_retard.cyl3_deg", 0.0),
            ("engine.timing_retard.cyl4_deg", -0.8),
            // DSG forks: 3rd engaged, 4th coming in on the other shaft.
            ("diag.02.15.1", 8.2),
            ("diag.02.15.2", 2.0 + 6.0 * (t * 0.35).sin().max(0.0)),
            ("diag.02.15.3", 0.2),
            ("diag.02.15.4", -0.1),
            ("diag.02.17.1", 4.1),
            ("diag.02.17.2", -4.0),
            ("diag.02.17.3", 4.4),
            ("diag.02.17.4", 3.9),
            ("diag.02.18.1", -4.3),
            ("diag.02.18.2", 4.2),
            ("diag.02.18.3", -4.1),
            ("diag.02.18.4", -4.5),
            ("trans.shifting", 0.0),
            ("vehicle.speed_kmh", 87.0),
        ];
        let at = now - Duration::from_millis(100 * (300 - i as u64));
        client.snapshot().values.take(
            vals.iter()
                .map(|(n, v)| Reading {
                    name: n.to_string(),
                    value: Some(if *n == "trans.shifting" {
                        Value::Bool(false)
                    } else {
                        Value::Num(*v)
                    }),
                    age_ms: 0,
                })
                .collect(),
            at,
        );
        if i % 3 == 0 {
            for k in 0..3 {
                pages.set_shown(k);
                let _ = pages.view(PageId::Data, &client.snapshot(), at);
            }
        }
    }
    pages.mark(3, now - Duration::from_secs(4));
    pages.set_shown(0);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("01-panel-daily");
    shots.show(&v, v.start, false);
    shots.save("02-panel-daily-day");
    touch_check(&shots);
    pages.set_shown(1);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("03-panel-boost");
    pages.set_shown(2);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("03b-panel-drive");

    // The menu, then the editor with a widget focused, then the widget's settings.
    pages.set_shown(0);
    let v = view(&pages);
    pages.press(PageId::Data, &v, &v.hit(v.flip).unwrap(), v.flip, &client);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("04-panel-menu");
    pages.press(PageId::Data, &v, &row(&v, "Edit panel"), 0, &client);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("05-panel-edit");
    pages.press(PageId::Data, &v, &Hit::Panel(0), v.start, &client);
    let v = view(&pages);
    shots.show(&v, 1, true);
    shots.save("05b-widget");
    shots.show(&v, 1, false);
    shots.save("05c-widget-day");
    pages.reset(PageId::Data);

    // Scratch: empty, then the picker.
    pages.set_shown(99);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("06-scratch");
    pages.press(PageId::Data, &v, &v.hit(v.flip).unwrap(), v.flip, &client);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("07-picker");
    pages.press(PageId::Data, &v, &row(&v, "Engine"), v.start, &client);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("08-picker-engine");
    pages.back(PageId::Data);
    let v = view(&pages);
    pages.press(PageId::Data, &v, &row(&v, "Module groups"), 0, &client);
    let v = view(&pages);
    shots.show(&v, 0, true);
    shots.save("08b-module-groups");
    pages.press(PageId::Data, &v, &row(&v, "01  Engine"), 0, &client);
    {
        let mut s = client.snapshot();
        s.diag.groups.insert(
            (1, 1),
            ["840 /min", "92 °C", "1.6 %", "6 °KW"]
                .iter()
                .map(|t| {
                    let (v, u) = t.split_once(' ').unwrap();
                    carpi_proto::GroupField {
                        value: v.parse().ok(),
                        text: v.into(),
                        unit: u.into(),
                    }
                })
                .collect(),
        );
    }
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("08c-module-group");
    pages.reset(PageId::Data);
    pages.set_shown(0);

    // A toast over the Data page.
    let v = view(&pages);
    shots.show(&v, -1, true);
    let t = shots.ui.global::<CarpiToast>();
    t.set_text("Debug logs 2026-11-08 #03".into());
    t.set_level(match ToastLevel::Info {
        ToastLevel::Info => 0,
        ToastLevel::Warn => 1,
        ToastLevel::Error => 2,
    });
    t.set_shown(true);
    shots.settle();
    shots.save("09-toast");
    t.set_shown(false);
    shots.settle();

    // Faults. The Car Integration tab is drawn by livi-carpi-shots.
    pages.open(PageId::Data, "faults");
    {
        use carpi_proto::{DiagAction, Dtc, JobEvent};
        let mut s = client.snapshot();
        s.diag.started(9, DiagAction::Scan);
        s.diag.event(
            9,
            JobEvent::Dtcs {
                module: 1,
                dtcs: vec![Dtc { code: 16684, status: 0x23 }, Dtc { code: 17965, status: 0x24 }],
            },
        );
        s.diag.event(9, JobEvent::Dtcs { module: 3, dtcs: vec![] });
        s.diag.event(
            9,
            JobEvent::Stage {
                stage: "failed".into(),
                message: "46 Central convenience".into(),
            },
        );
        s.diag.event(
            9,
            JobEvent::Stage {
                stage: "module".into(),
                message: "08 Auto HVAC".into(),
            },
        );
        s.diag.identity.insert(
            1,
            livi_carpi_ui::diag::Identity {
                part_no: Some("8P0907115H".into()),
                component: Some("2.0l R4/4V TFSI".into()),
                sw_version: Some("0040".into()),
                coding: Some("0403010A1C0700".into()),
                workshop: None,
            },
        );
    }
    let v = view(&pages);
    shots.show(&v, 3, true);
    shots.save("12-faults");
    pages.press(PageId::Data, &v, &row(&v, "01  Engine"), 3, &client);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("13-fault-module");
    pages.reset(PageId::Data);

    // Logs: idle with a last run, running, profiles, a profile, its channels, a picker.
    client.snapshot().datalog = Some(datalog());
    pages.open(PageId::Data, "logs");
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("14-logs");
    {
        let mut s = client.snapshot();
        let st = s.datalog.as_mut().unwrap();
        let mut r = st.last.take().unwrap();
        r.label = "2026-11-08 #04".into();
        r.elapsed_ms = 83_400;
        r.rows = 834;
        r.summary.clear();
        st.running = Some(r);
    }
    let v = view(&pages);
    shots.show(&v, v.start, false);
    shots.save("15-logs-running");
    let profiles = row(&v, "Profiles");
    pages.press(PageId::Data, &v, &profiles, 0, &client);
    let v = view(&pages);
    shots.show(&v, 0, true);
    shots.save("16-log-profiles");
    let boost = row(&v, "Boost");
    pages.press(PageId::Data, &v, &boost, 0, &client);
    let v = view(&pages);
    shots.show(&v, 1, true);
    shots.save("16b-log-profile");
    let ch = row(&v, "Channels");
    pages.press(PageId::Data, &v, &ch, 2, &client);
    let v = view(&pages);
    shots.show(&v, 1, true);
    shots.save("17-log-channels");
    pages.press(PageId::Data, &v, &Hit::Row(1), 1, &client);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("18-log-channel-picker");
}

/// Panels to show off every widget.
fn demo() -> livi_carpi_ui::panels::Panels {
    use livi_carpi_ui::panels::{Grid, Panel, Panels, Peak, Style, Widget};
    let mut boost = Widget::new(0, 0, Style::Gauge, &["engine.boost.actual"]).size(2, 3);
    boost.opts.relative = true;
    boost.opts.unit = Some("bar".into());
    boost.opts.peak = Peak::Hold;
    let mut graph = Widget::new(
        0,
        0,
        Style::Graph,
        &["engine.boost.actual", "engine.boost.specified"],
    )
    .size(3, 2);
    graph.opts.relative = true;
    graph.opts.unit = Some("bar".into());
    let mut pair = Widget::new(0, 1, Style::Pair, &["engine.boost.actual", "engine.boost.specified"]);
    pair.opts.relative = true;
    pair.opts.unit = Some("bar".into());
    Panels {
        panels: vec![
            Panel::new(
                "Daily",
                Grid::G4x3,
                vec![
                    boost,
                    Widget::new(2, 0, Style::Bar, &["engine.coolant_c"]).size(2, 1),
                    Widget::new(2, 1, Style::Bar, &["engine.oil_temp_c"]).size(2, 1),
                    Widget::new(2, 2, Style::Number, &["power.battery_v"]),
                    Widget::new(3, 2, Style::Number, &["vehicle.speed_kmh"]),
                ],
            ),
            Panel::new(
                "Boost",
                Grid::G3x3,
                vec![
                    graph,
                    Widget::new(0, 2, Style::Number, &["engine.n75_duty_pct"]),
                    Widget::new(1, 2, Style::Number, &["engine.load_pct"]),
                    Widget::new(
                        2,
                        2,
                        Style::Set,
                        &[
                            "engine.timing_retard.cyl1_deg",
                            "engine.timing_retard.cyl2_deg",
                            "engine.timing_retard.cyl3_deg",
                            "engine.timing_retard.cyl4_deg",
                        ],
                    ),
                ],
            ),
            Panel::new(
                "Drive",
                Grid::G3x2,
                vec![
                    Widget::new(0, 0, Style::Forks, &livi_carpi_ui::panels::FORKS).size(2, 1),
                    Widget::new(2, 0, Style::Gauge, &["engine.rpm"]).size(1, 2),
                    pair,
                    Widget::new(1, 1, Style::Number, &["vehicle.speed_kmh"]),
                ],
            ),
        ],
        shown: 0,
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
            labels: Vec::new(),
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
            marks: 0,
        }),
    }
}

/// A swipe and a tapped page dot reach `panel-flip`, as a finger would send them.
fn touch_check(shots: &Shots) {
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::LogicalPosition;
    let got = Rc::new(StdCell::new(Vec::<i32>::new()));
    let g = got.clone();
    shots.ui.global::<CarpiState>().on_panel_flip(move |d| {
        let mut v = g.take();
        v.push(d);
        g.set(v);
    });
    let w = shots.window.clone();
    let ev = |e: WindowEvent| w.dispatch_event(e);
    let step = || {
        NOW.set(NOW.get() + Duration::from_millis(16));
        slint::platform::update_timers_and_animations();
    };
    // Swipe left across the panel: the next one.
    let at = |x: f32| LogicalPosition::new(x, 240.0);
    ev(WindowEvent::PointerPressed { position: at(600.0), button: PointerEventButton::Left });
    for i in 1..=12 {
        step();
        ev(WindowEvent::PointerMoved { position: at(600.0 - 30.0 * i as f32) });
    }
    ev(WindowEvent::PointerReleased { position: at(240.0), button: PointerEventButton::Left });
    step();
    // Swipe right: the one before.
    ev(WindowEvent::PointerPressed { position: at(200.0), button: PointerEventButton::Left });
    for i in 1..=12 {
        step();
        ev(WindowEvent::PointerMoved { position: at(200.0 + 30.0 * i as f32) });
    }
    ev(WindowEvent::PointerReleased { position: at(560.0), button: PointerEventButton::Left });
    step();
    // A tap on a widget is not a swipe.
    ev(WindowEvent::PointerPressed { position: at(300.0), button: PointerEventButton::Left });
    step();
    ev(WindowEvent::PointerReleased { position: at(300.0), button: PointerEventButton::Left });
    step();
    let swipes = got.take();
    assert_eq!(swipes, vec![1, -1], "swipes");
    // The third page dot: dots sit centred at the bottom, 15 px apart.
    let st = shots.ui.global::<CarpiState>();
    let n = st.get_panel_dots() as f32;
    let width = n * 15.0 + 14.0;
    let left = 56.0 + 16.0 + (744.0 - 32.0 - width) / 2.0;
    let dot = LogicalPosition::new(left + 7.0 + 2.0 * 15.0 + 4.0, 480.0 - 4.0 - 8.0);
    ev(WindowEvent::PointerPressed { position: dot, button: PointerEventButton::Left });
    step();
    ev(WindowEvent::PointerReleased { position: dot, button: PointerEventButton::Left });
    step();
    let taps = got.take();
    assert_eq!(taps, vec![2 - st.get_panel_at()], "dot tap");
    println!("touch: swipes {swipes:?}, dot {taps:?}");
}
