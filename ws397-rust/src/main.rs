//! ws397-rust: e-reader / meeting-recorder firmware for the Waveshare
//! ESP32-S3-ePaper-3.97. Main task: e-paper + UI loop. Second task: rocker
//! input (debounced, with 2 s long-press detection).

use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio::{PinDriver, Pull},
    spi::{config::Config as SpiConfig, config::DriverConfig, SpiBusDriver, SpiDriver},
    units::*,
};
use std::sync::mpsc::{self, SyncSender};

mod board;
mod pipeline;
mod screens;
mod ssd1677;
mod ui;

use board::BookEntry;

use screens::{Action, AppState, Screen};
use ssd1677::Ssd1677;
use ui::Orientation;

pub const BTN_UP: usize = 4;
pub const BTN_PRESS: usize = 4 + 1; // GPIO5
pub const BTN_DOWN: usize = 4 + 2; // GPIO6
pub const BTN_BOOT: usize = 0;
/// Power-latch sense (schematic `PWR_OUT`, factory code
/// pcf85063_bsp.h:27). High = battery power latched on (rocker press).
pub const PWR_OUT: usize = 1;

/// Read a button level (0 = pressed). Direct GPIO access keeps the input
/// task free of peripheral ownership.
#[inline]
fn pressed(gpio: usize) -> bool {
    unsafe { esp_idf_svc::sys::gpio_get_level(gpio as i32) == 0 }
}

fn input_task(tx: SyncSender<Action>) -> ! {
    unsafe {
        use esp_idf_svc::sys;
        let mask = (1u64 << BTN_UP)
            | (1u64 << BTN_PRESS)
            | (1u64 << BTN_DOWN)
            | (1u64 << BTN_BOOT)
            | (1u64 << PWR_OUT);
        let conf = sys::gpio_config_t {
            pin_bit_mask: mask,
            mode: sys::gpio_mode_t_GPIO_MODE_INPUT,
            pull_up_en: sys::gpio_pullup_t_GPIO_PULLUP_ENABLE,
            pull_down_en: sys::gpio_pulldown_t_GPIO_PULLDOWN_DISABLE,
            intr_type: sys::gpio_int_type_t_GPIO_INTR_DISABLE,
        };
        sys::gpio_config(&conf);
    }
    struct Btn {
        gpio: usize,
        last_raw: bool,
        stable: bool,
        changed_at: u64,
    }
    let mut b = [
        Btn {
            gpio: BTN_UP,
            last_raw: false,
            stable: false,
            changed_at: 0,
        },
        Btn {
            gpio: BTN_PRESS,
            last_raw: false,
            stable: false,
            changed_at: 0,
        },
        Btn {
            gpio: BTN_DOWN,
            last_raw: false,
            stable: false,
            changed_at: 0,
        },
        Btn {
            gpio: BTN_BOOT,
            last_raw: false,
            stable: false,
            changed_at: 0,
        },
    ];
    let mut press_started: Option<u64> = None;
    let mut long_press_sent = false;
    let mut boot_pressed_at: Option<u64> = None;
    loop {
        let mut pressed_edge = [false; 4];
        let mut released_edge = [false; 4];
        for (i, bt) in b.iter_mut().enumerate() {
            let raw = pressed(bt.gpio);
            if raw != bt.last_raw {
                bt.last_raw = raw;
                bt.changed_at = now_ms();
            } else if raw != bt.stable && now_ms() - bt.changed_at >= 30 {
                bt.stable = raw;
                pressed_edge[i] = raw;
                released_edge[i] = !raw;
            }
        }
        let now = now_ms();

        // Rocker center: one LongPress after 2 s, or one Press on an earlier
        // release. The explicit debounced edges prevent a consumed long press
        // from becoming a second short press when released.
        if pressed_edge[1] {
            press_started = Some(now);
            long_press_sent = false;
        }
        if b[1].stable && !long_press_sent && press_started.is_some_and(|t| now - t >= 2000) {
            let _ = tx.send(Action::LongPress);
            long_press_sent = true;
        }
        if released_edge[1] {
            if !long_press_sent && press_started.is_some() {
                let _ = tx.send(Action::Press);
            }
            press_started = None;
            long_press_sent = false;
        }

        if pressed_edge[0] {
            let _ = tx.send(Action::Up);
        }
        if pressed_edge[2] {
            let _ = tx.send(Action::Down);
        }

        // BOOT: Back once on the debounced release.
        if pressed_edge[3] {
            boot_pressed_at = Some(now);
        }
        if released_edge[3] {
            if boot_pressed_at.is_some() {
                let _ = tx.send(Action::Back);
            }
            boot_pressed_at = None;
        }
        FreeRtos::delay_ms(10);
    }
}

fn now_ms() -> u64 {
    unsafe { esp_idf_svc::sys::esp_timer_get_time() as u64 / 1000 }
}

fn reader_surface(screen: Screen) -> bool {
    matches!(
        screen,
        Screen::Reader | Screen::ReaderMenu | Screen::FontSizeMenu
    )
}
fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let p = match esp_idf_svc::hal::peripherals::Peripherals::take() {
        Ok(p) => p,
        Err(e) => {
            log::error!("Peripherals::take: {e}");
            return;
        }
    };

    // Configure rocker + BOOT pins as pull-up inputs (active-low, vendor
    // button_bsp.c parity). Done via gpio_config for the 4 pins at once.
    {
        use esp_idf_svc::sys;
        unsafe {
            let mask = (1u64 << BTN_UP)
                | (1u64 << BTN_PRESS)
                | (1u64 << BTN_DOWN)
                | (1u64 << BTN_BOOT)
                | (1u64 << PWR_OUT);
            let conf = sys::gpio_config_t {
                pin_bit_mask: mask,
                mode: sys::gpio_mode_t_GPIO_MODE_INPUT,
                pull_up_en: sys::gpio_pullup_t_GPIO_PULLUP_ENABLE,
                pull_down_en: sys::gpio_pulldown_t_GPIO_PULLDOWN_DISABLE,
                intr_type: sys::gpio_int_type_t_GPIO_INTR_DISABLE,
            };
            sys::gpio_config(&conf);
        }
    }

    let driver = match SpiDriver::new(
        p.spi2,
        p.pins.gpio11, // SCLK
        p.pins.gpio12, // MOSI
        Option::<esp_idf_svc::hal::gpio::AnyInputPin>::None,
        &DriverConfig::new(),
    ) {
        Ok(d) => d,
        Err(e) => {
            log::error!("spi driver: {e}");
            return;
        }
    };
    let spi = match SpiBusDriver::new(driver, &SpiConfig::new().baudrate(20.MHz().into())) {
        Ok(s) => s,
        Err(e) => {
            log::error!("spi bus: {e}");
            return;
        }
    };
    // Raw input drivers (embedded-hal 1.0 InputPin); configured with pull-up,
    // interrupts disabled until the input task arms them.
    let mut busy_drv = PinDriver::input(p.pins.gpio3, Pull::Up).unwrap();
    busy_drv.disable_interrupt().ok();
    // EPD control as raw output drivers.
    let dc_drv = PinDriver::output(p.pins.gpio9).unwrap();
    let cs_drv = PinDriver::output(p.pins.gpio10).unwrap();
    let rst_drv = PinDriver::output(p.pins.gpio46).unwrap();
    // Rocker/BOOT pins are configured via raw gpio_config below and read via
    // gpio_get_level in the input task; the HAL handles are released.
    drop((p.pins.gpio4, p.pins.gpio5, p.pins.gpio6, p.pins.gpio0));

    let mut epd = Ssd1677::new(spi, cs_drv, dc_drv, rst_drv, busy_drv);
    log::info!("init display");
    epd.init(&mut FreeRtos);

    let books: Vec<BookEntry> = board::embedded_books();
    log::info!("embedded books: {}", books.len());

    let mut st = AppState {
        screen: Screen::Home,
        menu_idx: 0,
        library_idx: 0,
        reader_menu_idx: 0,
        font_menu_idx: 1,
        books,
        text: None,
        book_title: String::new(),
        layout: ui::Layout::new(Orientation::Portrait, 1),
        pages: Vec::new(),
        page_idx: 0,
        chapter_idx: 0,
    };

    let (tx, rx) = mpsc::sync_channel::<Action>(8);
    std::thread::Builder::new()
        .stack_size(4096)
        .spawn(move || input_task(tx))
        .expect("spawn input task");

    let mut pipe = pipeline::Pipeline::new();
    pipe.draw(&st, &mut epd, true, None);

    let mut n: u32 = 0;
    let mut budget = core::time::Duration::from_millis(1000);
    loop {
        // 1 Hz heartbeat on the serial console (M1 contract), while still
        // servicing rocker input. A full refresh can outlast a tick; the
        // budget below subtracts the overrun so heartbeats don't drift.
        let a = match mpsc::Receiver::recv_timeout(&rx, budget) {
            Ok(a) => Some(a),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(_) => break,
        };
        n += 1;
        log::info!("hello world (t={} s)", n);
        let tick = std::time::Instant::now();
        if let Some(a) = a {
            let was_screen = st.screen;
            let was_layout = (st.layout.orientation, st.layout.font_size);
            if screens::handle(&mut st, a) {
                let same_layout = was_layout == (st.layout.orientation, st.layout.font_size);
                let same_surface = was_screen == st.screen
                    || (reader_surface(was_screen) && reader_surface(st.screen));
                // Reader popups share the underlying page, so entering,
                // leaving, and moving within them only updates the popup box.
                let eligible = same_layout && same_surface;
                let region = if eligible {
                    pipeline::local_region(&st, a, true).and_then(|r| r)
                } else {
                    None
                };
                let region = region.map(|(rx0, ry0, rx1, ry1)| {
                    (
                        rx0.min(st.layout.width - 1),
                        ry0.min(st.layout.height - 1),
                        rx1.clamp(rx0 + 1, st.layout.width),
                        ry1.clamp(ry0 + 1, st.layout.height),
                    )
                });
                pipe.draw(&st, &mut epd, !eligible, region);
            }
        }
        let spent = tick.elapsed();
        budget = if spent >= core::time::Duration::from_millis(1000) {
            core::time::Duration::from_millis(20)
        } else {
            core::time::Duration::from_millis(1000) - spent
        };
    }
}
