#![windows_subsystem = "windows"]

use std::sync::{Arc, Mutex};
use std::time::Duration;
use gpui::prelude::*;
use gpui::*;

use asus_driver::{DriverError, SafeAsusDriver, HELPER_ARG, PIPE_ARG_PREFIX};
use fan_engine::{EngineTelemetrySnapshot, FanProfile, ThermalEngine};
use win_os::StartupManager;

/// Processes other than this application that also open `\\.\AsusSAIO`. Only
/// one of them can own the device — a second one silently reads `-1` from
/// every HealthyTable export.
///
/// Our own helper is deliberately absent: it *is* `asus-fan-app.exe`, and a
/// second GUI instance is ruled out by the single-instance mutex instead.
const COMPETING_PROCESSES: [&str; 2] = ["AsusFanControlGUI.exe", "AsusFanControl.exe"];

// White / orange palette.
const C_PAGE: u32 = 0xff_ff_ff;
const C_BORDER: u32 = 0xe5_e7_eb;
const C_BORDER_STRONG: u32 = 0xd1_d5_db;
const C_TEXT: u32 = 0x11_18_27;
const C_MUTED: u32 = 0x6b_72_80;
const C_ACCENT: u32 = 0xf9_73_16;
const C_ACCENT_DARK: u32 = 0xea_58_0c;
const C_ACCENT_SOFT: u32 = 0xff_ed_d5;
const C_CHIP: u32 = 0xf3_f4_f6;
const C_CHIP_TEXT: u32 = 0x37_41_51;
const C_OK: u32 = 0x16_a3_4a;
const C_OK_TEXT: u32 = 0x15_80_3d;
const C_DANGER: u32 = 0xdc_26_26;
const C_WARN_BG: u32 = 0xff_fb_eb;
const C_WARN_BORDER: u32 = 0xf5_9e_0b;
const C_WARN_TEXT: u32 = 0xb4_53_09;
const C_RPM: u32 = 0x05_96_69;

struct FanControlApp {
    telemetry: EngineTelemetrySnapshot,
    active_profile: FanProfile,
    engine: Option<Arc<ThermalEngine>>,
    is_autostart: bool,
    notice: Option<String>,
    manual_speed: u8,
}

impl FanControlApp {
    pub fn new(
        cx: &mut Context<Self>,
        engine: Option<Arc<ThermalEngine>>,
        initial_telemetry: EngineTelemetrySnapshot,
        notice: Option<String>,
    ) -> Self {
        let active_profile = engine
            .as_ref()
            .map(|e| e.active_profile())
            .unwrap_or(FanProfile::Balanced);

        let mut manual_speed = initial_telemetry.target_fan_percent.clamp(0, 100);
        if manual_speed == 0 {
            // Seed the track from the curve so it does not show a misleading 0%
            // for the first second, before the engine's first tick arrives.
            if let Some(curve) = active_profile.to_curve() {
                manual_speed = curve.evaluate(initial_telemetry.smoothed_cpu_temp) as u8;
            }
        }

        let is_autostart = StartupManager::is_autostart_enabled();

        if let Some(ref eng) = engine {
            // Subscribe BEFORE the first broadcast can fire, otherwise the
            // initial tick is lost and the UI sits on a stale snapshot.
            let rx = eng.subscribe_telemetry();
            cx.spawn(|this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let cx = cx.clone();
                async move {
                    let mut rx = rx;
                    loop {
                        match rx.recv().await {
                            Ok(snapshot) => {
                                let snap = snapshot.clone();
                                let _ = cx.update(|cx| {
                                    let _ = this.update(cx, |view, cx| {
                                        view.telemetry = snap.clone();
                                        if !matches!(view.active_profile, FanProfile::Manual(_)) {
                                            view.manual_speed = snap.target_fan_percent;
                                        }
                                        cx.notify();
                                    });
                                });
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            })
            .detach();
        }

        Self {
            telemetry: initial_telemetry,
            active_profile,
            engine,
            is_autostart,
            notice,
            manual_speed,
        }
    }

    fn select_profile(&mut self, profile: FanProfile, cx: &mut Context<Self>) {
        self.active_profile = profile.clone();
        if let Some(ref eng) = self.engine {
            eng.set_profile(profile);
        }
        cx.notify();
    }

    fn set_manual_speed(&mut self, percent: u8, cx: &mut Context<Self>) {
        self.manual_speed = percent.clamp(0, 100);
        self.select_profile(FanProfile::Manual(self.manual_speed), cx);
    }

    fn toggle_autostart(&mut self, cx: &mut Context<Self>) {
        if let Ok(exe_path) = std::env::current_exe() {
            if self.is_autostart {
                let _ = StartupManager::disable_autostart();
                self.is_autostart = false;
            } else {
                // Standalone: the EXE elevates itself, so point at it directly.
                let _ = StartupManager::enable_autostart(&exe_path, "");
                self.is_autostart = true;
            }
            cx.notify();
        }
    }

    fn preset_button(&self, pct: u8, cx: &mut Context<Self>) -> impl IntoElement {
        let is_active = self.manual_speed == pct;
        div()
            .id(ElementId::named_usize("btn-pct", pct as usize))
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .bg(if is_active { rgb(C_ACCENT) } else { rgb(C_CHIP) })
            .hover(|s| {
                s.bg(if is_active {
                    rgb(C_ACCENT_DARK)
                } else {
                    rgb(C_BORDER)
                })
            })
            .text_color(if is_active {
                rgb(0xff_ff_ff)
            } else {
                rgb(C_CHIP_TEXT)
            })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.set_manual_speed(pct, cx);
            }))
            .child(if pct == 0 {
                "Off 0%".to_string()
            } else {
                format!("{}%", pct)
            })
    }

    fn profile_button(
        &self,
        label: &'static str,
        target: FanProfile,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(label)
            .px_3()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .bg(if active { rgb(C_ACCENT) } else { rgb(C_CHIP) })
            .hover(|s| {
                s.bg(if active {
                    rgb(C_ACCENT_DARK)
                } else {
                    rgb(C_BORDER)
                })
            })
            .text_color(if active {
                rgb(0xff_ff_ff)
            } else {
                rgb(C_CHIP_TEXT)
            })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                this.select_profile(target.clone(), cx);
            }))
            .child(label)
    }
}

impl Render for FanControlApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let telemetry = self.telemetry.clone();
        let fan_rpms = telemetry.fan_rpms.clone();

        let cpu_temp_display = if telemetry.raw_cpu_temp == 0 && self.engine.is_none() {
            "--°C".to_string()
        } else {
            format!("{}°C", telemetry.raw_cpu_temp)
        };
        let smoothed_display = format!("{:.1}°C", telemetry.smoothed_cpu_temp);
        let requested_display = format!("{}%", telemetry.target_fan_percent);
        let applied_display = match telemetry.applied_fan_percent {
            Some(v) => format!("{}%", v),
            None if self.engine.is_none() => "—".to_string(),
            None => "not confirmed".to_string(),
        };

        let is_emergency = telemetry.is_emergency_active;
        let has_error = self.notice.is_some() || telemetry.last_error.is_some();
        let is_manual = matches!(self.active_profile, FanProfile::Manual(_));

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(C_PAGE))
            .text_color(rgb(C_TEXT))
            .p_6()
            .gap_4()
            // Header
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pb_3()
                    .border_b_1()
                    .border_color(rgb(C_BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(C_ACCENT))
                                    .child("ASUS FAN CONTROL"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .bg(if is_emergency {
                                        rgb(C_DANGER)
                                    } else if has_error {
                                        rgb(C_WARN_BG)
                                    } else {
                                        rgb(C_OK)
                                    })
                                    .text_color(if has_error && !is_emergency {
                                        rgb(C_WARN_TEXT)
                                    } else {
                                        rgb(0xff_ff_ff)
                                    })
                                    .child(if is_emergency {
                                        "EMERGENCY"
                                    } else if has_error {
                                        "CHECK NOTICE"
                                    } else {
                                        "ACTIVE"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(C_MUTED))
                            // The window itself is a normal elevated process; only
                            // the helper it talks to over the pipe is SYSTEM.
                            .child(if self.engine.is_some() {
                                "SYSTEM helper attached".to_string()
                            } else {
                                "no SYSTEM helper".to_string()
                            }),
                    ),
            )
            // Notice banner with relaunch action
            .when_some(self.notice.clone(), |this, err| {
                this.child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(rgb(C_WARN_BG))
                        .border_1()
                        .border_color(rgb(C_WARN_BORDER))
                        .flex()
                        .justify_between()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(C_WARN_TEXT))
                                        .child("HARDWARE ACCESS NOTICE"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(C_CHIP_TEXT))
                                        .child(err),
                                ),
                        )
                        .child(
                            div()
                                .id("btn-relaunch")
                                .px_3()
                                .py_1p5()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(rgb(C_ACCENT))
                                .hover(|s| s.bg(rgb(C_ACCENT_DARK)))
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0xff_ff_ff))
                                .on_mouse_down(MouseButton::Left, |_, _, _| {
                                    if let Ok(exe) = std::env::current_exe() {
                                        // Hand this process over so the replacement does
                                        // not report us as a competing instance.
                                        let parent_arg =
                                            format!("--parent-pid={}", std::process::id());
                                        let mut cmd = std::process::Command::new(&exe);
                                        cmd.arg(parent_arg);
                                        if let Some(dir) = exe.parent() {
                                            cmd.current_dir(dir);
                                        }
                                        let _ = cmd.spawn();
                                    }
                                    std::process::exit(0);
                                })
                                .child("Restart application"),
                        ),
                )
            })
            // Main content: 2 columns
            .child(
                div()
                    .flex()
                    .gap_4()
                    .flex_1()
                    // Left: telemetry
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .w_1_2()
                            // CPU temperature card
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .p_4()
                                    .rounded_lg()
                                    .bg(rgb(0xff_ff_ff))
                                    .border_1()
                                    .border_color(rgb(C_BORDER))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(C_MUTED))
                                            .child("CPU PACKAGE TEMPERATURE"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_baseline()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_2xl()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgb(C_ACCENT))
                                                    .child(cpu_temp_display),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(C_MUTED))
                                                    .child(format!("(Smoothed: {})", smoothed_display)),
                                            ),
                                    ),
                            )
                            // Fan tachometer card
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .p_4()
                                    .rounded_lg()
                                    .bg(rgb(0xff_ff_ff))
                                    .border_1()
                                    .border_color(rgb(C_BORDER))
                                    .child(
                                        div()
                                            .flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(C_MUTED))
                                                    .child("FAN SPEED TACHOMETERS"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(C_MUTED))
                                                    .child("live @ 1 Hz"),
                                            ),
                                    )
                                    .children(if fan_rpms.is_empty() {
                                        vec![div()
                                            .pt_3()
                                            .text_sm()
                                            .text_color(rgb(C_MUTED))
                                            .child("Waiting for the first tachometer reading…")]
                                    } else {
                                        fan_rpms
                                            .iter()
                                            .enumerate()
                                            .map(|(idx, rpm)| {
                                                div()
                                                    .flex()
                                                    .justify_between()
                                                    .items_baseline()
                                                    .pt_2()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(rgb(C_MUTED))
                                                            .child(format!(
                                                                "Fan #{} ({})",
                                                                idx,
                                                                if idx == 0 { "CPU" } else { "GPU" }
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_2xl()
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(rgb(C_RPM))
                                                            .child(format!("{} RPM", rpm)),
                                                    )
                                            })
                                            .collect()
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .justify_between()
                                            .items_center()
                                            .pt_3()
                                            .mt_2()
                                            .border_t_1()
                                            .border_color(rgb(C_BORDER))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(C_MUTED))
                                                    .child("Fan Duty"),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .gap_3()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(rgb(C_MUTED))
                                                            .child(format!("req {}", requested_display)),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(rgb(C_OK_TEXT))
                                                            .child(format!("hw {}", applied_display)),
                                                    ),
                                            ),
                                    ),
                            ),
                    )
                    // Right: manual control (always available)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .p_4()
                            .rounded_lg()
                            .bg(rgb(0xff_ff_ff))
                            .border_1()
                            .border_color(rgb(C_BORDER))
                            .w_1_2()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(C_MUTED))
                                            .child("MANUAL FAN SPEED CONTROL"),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_full()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .bg(if is_manual {
                                                rgb(C_ACCENT_SOFT)
                                            } else {
                                                rgb(C_CHIP)
                                            })
                                            .text_color(if is_manual {
                                                rgb(C_ACCENT_DARK)
                                            } else {
                                                rgb(C_MUTED)
                                            })
                                            .child(if is_manual { "Direct PWM" } else { "Curve" }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(C_TEXT))
                                    .child(self.active_profile.name()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(C_MUTED))
                                    .child("Click a preset or the track to take manual control:"),
                            )
                            // 0..100 click track
                            .child(
                                div()
                                    .id("speed-track")
                                    .flex()
                                    .rounded_md()
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(rgb(C_BORDER_STRONG))
                                    .children((0..=100u8).map(|v| {
                                        let filled = v <= self.manual_speed;
                                        div()
                                            .id(ElementId::named_usize("track-cell", v as usize))
                                            .flex_1()
                                            .py_3()
                                            .cursor_pointer()
                                            .bg(if filled { rgb(C_ACCENT) } else { rgb(C_CHIP) })
                                            .hover(|s| {
                                                s.bg(if filled {
                                                    rgb(C_ACCENT_DARK)
                                                } else {
                                                    rgb(C_BORDER_STRONG)
                                                })
                                            })
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                                this.set_manual_speed(v, cx);
                                            }))
                                    })),
                            )
                            // Selected duty readout
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(C_MUTED))
                                            .child("Selected duty"),
                                    )
                                    .child(
                                        div()
                                            .text_2xl()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(C_ACCENT))
                                            .child(format!("{}%", self.manual_speed)),
                                    ),
                            )
                            // Presets — wrap so they never overflow the column
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_1p5()
                                    .children([
                                        self.preset_button(0, cx),
                                        self.preset_button(25, cx),
                                        self.preset_button(40, cx),
                                        self.preset_button(50, cx),
                                        self.preset_button(60, cx),
                                        self.preset_button(70, cx),
                                        self.preset_button(80, cx),
                                        self.preset_button(90, cx),
                                        self.preset_button(100, cx),
                                    ]),
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .text_xs()
                                    .pt_1()
                                    .child(
                                        div()
                                            .text_color(rgb(C_MUTED))
                                            .child(format!("Requested: {}", requested_display)),
                                    )
                                    .child(
                                        div()
                                            .text_color(rgb(C_OK_TEXT))
                                            .child(format!("Hardware: {}", applied_display)),
                                    ),
                            )
                            .when_some(telemetry.last_error.clone(), |this, err| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(C_DANGER))
                                        .child(format!("Error: {}", err)),
                                )
                            })
                            // Active curve, read-only
                            .when(!is_manual, |this| {
                                this.child(
                                    div()
                                        .pt_2()
                                        .mt_1()
                                        .border_t_1()
                                        .border_color(rgb(C_BORDER))
                                        .children(
                                            self.active_profile
                                                .to_curve()
                                                .map(|c| {
                                                    c.points
                                                        .iter()
                                                        .map(|p| {
                                                            div()
                                                                .flex()
                                                                .justify_between()
                                                                .text_xs()
                                                                .child(format!("{:.0}°C", p.temp_c))
                                                                .child(
                                                                    div()
                                                                        .text_color(rgb(C_ACCENT_DARK))
                                                                        .child(format!("{:.0}% Duty", p.fan_percent)),
                                                                )
                                                        })
                                                        .collect::<Vec<_>>()
                                                })
                                                .unwrap_or_else(|| {
                                                    vec![div()
                                                        .text_xs()
                                                        .text_color(rgb(C_MUTED))
                                                        .child("Using factory ASUS BIOS firmware curve")]
                                                }),
                                        ),
                                )
                            }),
                    ),
            )
            // Footer: profile buttons & startup toggle
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .justify_between()
                    .items_center()
                    .pt_3()
                    .border_t_1()
                    .border_color(rgb(C_BORDER))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(self.profile_button("Silent", FanProfile::Silent, self.active_profile == FanProfile::Silent, cx))
                            .child(self.profile_button("Balanced", FanProfile::Balanced, self.active_profile == FanProfile::Balanced, cx))
                            .child(self.profile_button("Turbo", FanProfile::Turbo, self.active_profile == FanProfile::Turbo, cx))
                            .child(
                                div()
                                    .id("profile-manual")
                                    .px_3()
                                    .py_1p5()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .bg(if is_manual { rgb(C_ACCENT) } else { rgb(C_CHIP) })
                                    .hover(|s| {
                                        s.bg(if is_manual {
                                            rgb(C_ACCENT_DARK)
                                        } else {
                                            rgb(C_BORDER)
                                        })
                                    })
                                    .text_color(if is_manual {
                                        rgb(0xff_ff_ff)
                                    } else {
                                        rgb(C_CHIP_TEXT)
                                    })
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        let spd = this.manual_speed;
                                        this.select_profile(FanProfile::Manual(spd), cx);
                                    }))
                                    .child("Manual"),
                            )
                            .child(self.profile_button("BIOS Default", FanProfile::BiosDefault, self.active_profile == FanProfile::BiosDefault, cx)),
                    )
                    .child(
                        div()
                            .id("toggle-autostart")
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(rgb(C_CHIP))
                            .hover(|s| s.bg(rgb(C_BORDER)))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if self.is_autostart { rgb(C_OK_TEXT) } else { rgb(C_MUTED) })
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.toggle_autostart(cx);
                            }))
                            .child(format!("Auto-Start: {}", if self.is_autostart { "Enabled" } else { "Disabled" })),
                    ),
            )
    }
}

fn data_dir() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.to_path_buf();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Appending log file — the old implementation truncated `app.log` on every
/// write, so only the final message ever survived.
#[derive(Clone)]
struct AppLogWriter(Arc<Mutex<std::fs::File>>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for AppLogWriter {
    type Writer = AppLogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl std::io::Write for AppLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).flush()
    }
}

fn init_logging() {
    use tracing_subscriber::prelude::*;
    let path = data_dir().join("app.log");
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let writer = AppLogWriter(Arc::new(Mutex::new(file)));
        tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new("info"))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false),
            )
            .init();
    }

    tracing::info!(
        "---- asus-fan-app start: pid={}, sid={}, as_system={}, cwd={:?}, exe={:?} ----",
        std::process::id(),
        asus_driver::token_sid_string(),
        asus_driver::is_local_system(),
        std::env::current_dir().ok(),
        std::env::current_exe().ok()
    );
}

/// Lists other processes that would fight us for `\\.\AsusSAIO`.
///
/// `exclude` is our own PID. This application's own helper is never reported:
/// it *is* `asus-fan-app.exe`, and a second copy of the GUI is already ruled
/// out by the single-instance mutex.
fn competing_processes(exclude: &[u32]) -> Vec<String> {
    let output = match std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("tasklist failed: {}", e);
            return Vec::new();
        }
    };

    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split("\",\"").collect();
            if cols.len() < 2 {
                return None;
            }
            let name = cols[0].trim_matches('"');
            let pid: u32 = cols[1].trim_matches('"').parse().ok()?;
            if exclude.contains(&pid) {
                return None;
            }
            if COMPETING_PROCESSES.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                Some(format!("{} (PID {})", name, pid))
            } else {
                None
            }
        })
        .collect()
}

/// Extracts `--pipe=<name>` handed to the SYSTEM helper process.
fn pipe_name_from_args() -> Option<String> {
    std::env::args()
        .find_map(|arg| arg.strip_prefix(PIPE_ARG_PREFIX).map(|s| s.to_string()))
}

fn notice_for_error(err: &DriverError) -> String {
    match err {
        DriverError::ElevationRequired => {
            "Not running as SYSTEM — \\\\.\\AsusSAIO rejects this process, so RPM reads \
             return -1 and writes do nothing. Restart the application so it can start \
             its SYSTEM helper."
                .to_string()
        }
        other => format!("{}", other),
    }
}

/// Requests administrator rights (UAC) for the single-instance guard only — the
/// fan hardware itself is handled by the SYSTEM helper.
///
/// Returns `true` when this process should continue. `false` means either an
/// elevated copy has taken over, or elevation failed (already reported).
fn ensure_elevated() -> bool {
    if asus_driver::is_elevated() {
        return true;
    }

    tracing::info!("Not elevated; requesting administrator privileges");
    // Hand the single-instance lock back first: the elevated copy acquires it
    // immediately after consent, and must not see it still held by us.
    asus_driver::release_single_instance();
    match asus_driver::relaunch_elevated() {
        Ok(()) => false,
        Err(e) => {
            tracing::error!("Elevation failed: {}", e);
            asus_driver::show_error(
                "Asus Fan Control",
                &format!(
                    "Administrator privileges are required to start the SYSTEM helper.\n\n{}\n\n\
                     The application will now exit.",
                    e
                ),
            );
            false
        }
    }
}

/// Registers and starts the SYSTEM helper task, then attaches to its pipe.
fn attach_to_helper() -> Result<SafeAsusDriver, DriverError> {
    let exe = std::env::current_exe()
        .map_err(|e| DriverError::DriverNotReady {
            reason: format!("cannot resolve own path: {}", e),
        })?;

    let pipe = asus_driver::make_pipe_name();
    tracing::info!("Starting SYSTEM helper on {}", pipe);
    asus_driver::start_system_task(&exe, &[HELPER_ARG, &format!("{}{}", PIPE_ARG_PREFIX, pipe)])
        .map_err(|e| DriverError::DriverNotReady {
            reason: format!("could not start helper: {}", e),
        })?;

    match SafeAsusDriver::connect(&pipe) {
        Ok(d) => Ok(d),
        Err(e) => {
            asus_driver::stop_system_task();
            Err(e)
        }
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let path = data_dir().join("panic.log");
        let _ = std::fs::write(path, format!("PANIC: {:?}\n", info));
    }));

    init_logging();

    // Headless SYSTEM worker: never opens a window, never reaches the GUI path.
    if let Some(pipe) = pipe_name_from_args() {
        std::process::exit(asus_driver::helper::run(&pipe));
    }

    // One GUI per machine. A second copy would delete the running helper's
    // scheduled task and start a competing helper against the same EC.
    if !asus_driver::acquire_single_instance() {
        asus_driver::show_error(
            "Asus Fan Control",
            "Asus Fan Control is already running.\n\nSwitch to the existing window instead \
             of starting a second copy.",
        );
        return;
    }

    if !ensure_elevated() {
        asus_driver::release_single_instance();
        return;
    }

    // Warn before touching hardware if another fan tool already owns the device.
    let conflicts = competing_processes(&[std::process::id()]);
    let mut notice: Option<String> = if conflicts.is_empty() {
        None
    } else {
        Some(format!(
            "{} is already running and holds \\\\.\\AsusSAIO. Close it — only one fan \
             control app can talk to the EC at a time.",
            conflicts.join(", ")
        ))
    };

    let mut telemetry = EngineTelemetrySnapshot::placeholder();
    let engine: Option<Arc<ThermalEngine>> = match attach_to_helper() {
        Ok(driver) => {
            let init_temp = driver.lock().get_cpu_temperature().unwrap_or(0);
            let init_rpms = driver.lock().get_all_fan_rpms().unwrap_or_default();
            tracing::info!(
                "Driver up: {} fan(s), cpu={}°C, rpms={:?}",
                driver.lock().fan_count(),
                init_temp,
                init_rpms
            );
            telemetry = EngineTelemetrySnapshot {
                raw_cpu_temp: init_temp,
                smoothed_cpu_temp: init_temp as f32,
                target_fan_percent: 0,
                applied_fan_percent: None,
                fan_rpms: init_rpms,
                is_emergency_active: false,
                active_profile_name: "Balanced".to_string(),
                last_error: None,
            };

            Some(Arc::new(ThermalEngine::new(driver)))
        }
        Err(e) => {
            tracing::error!("Driver initialization failed: {}", e);
            let msg = notice_for_error(&e);
            notice = Some(match notice {
                Some(existing) => format!("{}\n\n{}", existing, msg),
                None => msg,
            });
            None
        }
    };

    // FanControlApp::new subscribes to telemetry when the window is built, so
    // start() must run *after* open_window or the first broadcast tick is lost.
    let engine_for_shutdown = engine.clone();
    let mut engine_to_start = engine.clone();
    let mut engine_to_run = engine.clone();
    let telemetry_for_window = telemetry.clone();
    let notice_for_window = notice.clone();

    Application::new().run(move |cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(880.0), px(620.0)), cx);
        let win = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Asus Fan Control".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| {
                let engine = engine_to_start.take();
                cx.new(move |cx| {
                    FanControlApp::new(cx, engine, telemetry_for_window, notice_for_window)
                })
            },
        );

        match win {
            Ok(_) => tracing::info!("Window opened successfully."),
            Err(e) => tracing::error!("Window creation error: {:?}", e),
        }

        if let Some(eng) = engine_to_run.take() {
            eng.start(Duration::from_millis(1000));
        }
    });

    tracing::info!("UI exited; shutting down thermal engine.");
    if let Some(eng) = &engine_for_shutdown {
        eng.stop();
    }
    // Dropping the engine closes the control pipe. The helper detects the
    // disconnect, returns the fans to BIOS control and deletes its own
    // scheduled task — so a crashed or killed GUI cannot leave a latched duty
    // or a resident SYSTEM process behind.
    drop(engine_for_shutdown);
    std::thread::sleep(Duration::from_millis(500));
    asus_driver::stop_system_task();
    asus_driver::release_single_instance();
}
