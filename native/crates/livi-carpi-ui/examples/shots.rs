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
    import { CarpiView, CarpiState, CarpiLayout, CarpiKind, CarpiPill, CarpiRow, CarpiCell, CarpiSpot, CarpiPoint, CarpiTick, CarpiToast, CarpiToastView } from "../ui/carpi.slint";
    export { Theme, Focus, FocusZone, CarpiState, CarpiLayout, CarpiKind, CarpiPill, CarpiRow, CarpiCell, CarpiSpot, CarpiPoint, CarpiTick, CarpiToast }

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

    // The Data page, at night and by day, and with the 4×4 page.
    let v = view(&pages);
    shots.show(&v, 0, true);
    shots.save("01-data");
    shots.show(&v, 0, false);
    shots.save("02-data-day");
    pages.side(PageId::Data, &v, v.hit(0).as_ref(), 1, 0, &client);
    let v = view(&pages);
    shots.show(&v, 1, true);
    shots.save("03-data-4x4");
    pages.side(PageId::Data, &v, v.hit(1).as_ref(), -1, 1, &client);

    // The editor: first slot focused, then a row of the page settings.
    let v = view(&pages);
    let edit = Hit::Pill(v.pills.len() - 1);
    pages.press(PageId::Data, &v, &edit, 0, &client);
    let v = view(&pages);
    shots.show(&v, v.start + 4, true);
    shots.save("04-edit");
    shots.show(&v, v.start + 4, false);
    shots.save("05-edit-day");
    let first_row = (v.pills.len() + v.cells.len()) as i32;
    shots.show(&v, first_row + 1, true);
    shots.save("05b-edit-page-rows");

    // The picker: level 1, then Engine.
    pages.press(PageId::Data, &v, &Hit::Cell(4), v.start + 4, &client);
    let v = view(&pages);
    shots.show(&v, 0, true);
    shots.save("06a-picker-top");
    shots.show(&v, v.start, true);
    shots.save("06-picker");
    pages.press(PageId::Data, &v, &row(&v, "Engine"), v.start, &client);
    let v = view(&pages);
    shots.show(&v, v.start, true);
    shots.save("07-picker-engine");
    shots.show(&v, v.start, false);
    shots.save("08-picker-engine-day");
    pages.reset(PageId::Data);

    // A toast over the Data page.
    let v = view(&pages);
    shots.show(&v, -1, true);
    let t = shots.ui.global::<CarpiToast>();
    t.set_text("Capture 2026-11-08 #03".into());
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

    // Diagnostics. The Car Integration tab is drawn by livi-carpi-shots.
    let v = pages.view(PageId::Diagnostics, &client.snapshot(), now);
    shots.show(&v, 1, true);
    shots.save("12-diagnostics");

    // Logging: idle with a last run, running, a profile, its channels, a channel's picker.
    client.snapshot().datalog = Some(datalog());
    pages.open(PageId::Car, "logging");
    let v = pages.view(PageId::Car, &client.snapshot(), now);
    shots.show(&v, v.start, true);
    shots.save("14-logging");
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
    let v = pages.view(PageId::Car, &client.snapshot(), now);
    shots.show(&v, v.start, false);
    shots.save("15-logging-running");
    let boost = row(&v, "Boost");
    pages.press(PageId::Car, &v, &boost, 0, &client);
    let v = pages.view(PageId::Car, &client.snapshot(), now);
    shots.show(&v, 1, true);
    shots.save("16-log-profile");
    let ch = row(&v, "Channels");
    pages.press(PageId::Car, &v, &ch, 2, &client);
    let v = pages.view(PageId::Car, &client.snapshot(), now);
    shots.show(&v, 1, true);
    shots.save("17-log-channels");
    pages.press(PageId::Car, &v, &Hit::Row(1), 1, &client);
    let v = pages.view(PageId::Car, &client.snapshot(), now);
    shots.show(&v, v.start, true);
    shots.save("18-log-channel-picker");
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
