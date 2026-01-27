#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use alloc::{boxed::Box, rc::Rc};
use core::cell::RefCell;
use embedded_hal_bus::spi::RefCellDevice;
use embedded_sdmmc::{Mode, SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use esp_backtrace as _;
use esp_hal::{
    Blocking,
    clock::CpuClock,
    delay::Delay,
    gpio::{
        Level, Output, OutputPin,
        interconnect::{PeripheralInput, PeripheralOutput},
    },
    main,
    peripherals::Peripherals,
    rng::Rng,
    spi::master::Spi,
    time::{Instant, Rate},
    timer::timg::TimerGroup,
};
use esp_println::println;
use slint::{
    PhysicalPosition, PhysicalSize, PlatformError,
    platform::{
        Platform, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        update_timers_and_animations,
    },
};
use smoltcp::iface::SocketStorage;

use crate::{
    display_screen::init_ili9341_display, http_client::{HttpClient, Method}, secrets::{TEST_ADDRESS, TEST_IP, WIFI_PASSWORD, WIFI_SSID}, slint_renderer::SlintRenderer, touch_input::{TouchInputProvider, TouchInputResponse, Xpt2046TouchInput}, wifi::{Wifi, obtain_ip}
};

extern crate alloc;

mod display_screen;
mod secrets;
mod slint_renderer;
mod touch_input;
mod wifi;
mod http_client;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

slint::include_modules!();

fn handle_input(
    window: &Rc<MinimalSoftwareWindow>,
    touch_input_provider: &mut impl TouchInputProvider,
) -> Result<(), PlatformError> {
    if let Ok(x) = touch_input_provider.get_input() {
        match x {
            TouchInputResponse::Moved { x, y } => {
                let logical = PhysicalPosition::new(x, y).to_logical(window.scale_factor());
                window.try_dispatch_event(WindowEvent::PointerMoved { position: logical })?;
            }
            TouchInputResponse::Pressed { x, y } => {
                let logical = PhysicalPosition::new(x, y).to_logical(window.scale_factor());
                window.try_dispatch_event(WindowEvent::PointerPressed {
                    position: logical,
                    button: PointerEventButton::Left,
                })?;
            }
            TouchInputResponse::Released { x, y } => {
                window.try_dispatch_event(WindowEvent::PointerReleased {
                    position: PhysicalPosition::new(x, y).to_logical(window.scale_factor()),
                    button: PointerEventButton::Left,
                })?;
                window.try_dispatch_event(WindowEvent::PointerExited)?;
            }
            TouchInputResponse::NoInput => (),
        }
    }

    Ok(())
}

struct DummyTime;
impl TimeSource for DummyTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp::from_calendar(2024, 1, 1, 0, 0, 0).unwrap()
    }
}

fn init_sd_card<'a>(spi: &'a RefCell<Spi<'a, Blocking>>, sd_cs_pin: impl OutputPin + 'a) {
    let sd_cs = Output::new(sd_cs_pin, Level::High, Default::default());
    let sd_spi_dev = RefCellDevice::new_no_delay(spi, sd_cs).unwrap();

    let sd = SdCard::new(sd_spi_dev, Delay::new());
    let controller = VolumeManager::new(sd, DummyTime);

    let mut attempt = 0;
    let max_attempts = 5;
    loop {
        attempt += 1;
        match controller.open_volume(VolumeIdx(0)) {
            Ok(volume) => {
                if let Ok(root) = volume.open_root_dir() {
                    if let Ok(file) = root.open_file_in_dir("HELLO.TXT", Mode::ReadOnly) {
                        let mut buf = [0u8; 64];
                        if let Ok(n) = file.read(&mut buf) {
                            esp_println::println!("SD: Read {} bytes: {:?}", n, &buf[..n]);
                        }
                    }
                }
                break;
            }
            Err(e) => {
                esp_println::println!("SD: Attempt {}/{} failed: {:?}", attempt, max_attempts, e);
                if attempt >= max_attempts {
                    break;
                }
                Delay::new().delay_millis(50u32);
            }
        }
    }
}

struct EspBackend {
    window: RefCell<Option<Rc<MinimalSoftwareWindow>>>,
    peripherals: RefCell<Option<Peripherals>>,
}

impl Default for EspBackend {
    fn default() -> Self {
        Self {
            window: RefCell::new(None),
            peripherals: RefCell::new(None),
        }
    }
}

fn create_spi<'a>(
    spi: impl esp_hal::spi::master::Instance + 'a,
    sck: impl PeripheralOutput<'a>,
    mosi: impl PeripheralOutput<'a>,
    miso: impl PeripheralInput<'a>,
    frequency: Rate,
) -> Spi<'a, Blocking> {
    Spi::<esp_hal::Blocking>::new(
        spi,
        esp_hal::spi::master::Config::default()
            .with_frequency(frequency)
            .with_mode(esp_hal::spi::Mode::_0),
    )
    .unwrap()
    .with_sck(sck)
    .with_mosi(mosi)
    .with_miso(miso)
}

impl Platform for EspBackend {
    fn duration_since_start(&self) -> core::time::Duration {
        core::time::Duration::from_millis(Instant::now().duration_since_epoch().as_millis())
    }

    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        self.window.replace(Some(w.clone()));
        Ok(w)
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let peripherals = self
            .peripherals
            .borrow_mut()
            .take()
            .expect("Peripherals already taken");

        let timg0 = TimerGroup::new(peripherals.TIMG0);
        let rng = Rng::new();
        esp_rtos::start(timg0.timer0);

        let radio_init = esp_radio::init().unwrap();
        let mut sockets_buf: [SocketStorage; 4] = Default::default();
        let mut wifi = Wifi::new(
            peripherals.WIFI,
            &radio_init,
            WIFI_SSID,
            WIFI_PASSWORD,
        );
        wifi.initialize();
        let mut stack = Rc::new(wifi::build_stack(wifi.interfaces.sta, &mut sockets_buf, || Instant::now().duration_since_epoch().as_millis(), rng.random()));
        obtain_ip(&mut stack);

        let mut http = HttpClient::new(
            stack.clone(),
            TEST_ADDRESS,
            TEST_IP,
        );
        let response = http.request(
            Method::Get,
            "/api/Tags/tag-crime",
            None,
            10,
        ).unwrap();
        println!("{}", response);

        let mut http = HttpClient::new(
            stack.clone(),
            TEST_ADDRESS,
            TEST_IP,
        );
        let response = http.request(
            Method::Delete,
            "/api/Tags/tag-crime",
            None,
            10,
        ).unwrap();
        println!("{}", response);

        let mut http = HttpClient::new(
            stack.clone(),
            TEST_ADDRESS,
            TEST_IP,
        );
        let body = br#"{"hello":"esp32"}"#;
        let response = http.request(
            Method::Post,
            "/api/Tags/tag-crime",
            Some(body),
            10,
        )?;
        println!("{}", response);

        let mut http = HttpClient::new(
            stack.clone(),
            TEST_ADDRESS,
            TEST_IP,
        );
        let body = br#"{"hello":"esp32"}"#;
        let response = http.request(
            Method::Put,
            "/api/Tags/tag-crime",
            Some(body),
            10,
        )?;
        println!("{}", response);

        let mut http = HttpClient::new(
            stack.clone(),
            TEST_ADDRESS,
            TEST_IP,
        );
        let body = br#"{"hello":"esp32"}"#;
        let response = http.request(
            Method::Patch,
            "/api/Tags/tag-crime",
            Some(body),
            10,
        )?;
        println!("{}", response);


        //SD requires 100kHz-400kHz
        //Display in order to be fast needs like 40MHz
        //XPT 2046 can have around 4MHz - it doesn't work on values that are too big
        let fast_spi = create_spi(
            peripherals.SPI3,
            peripherals.GPIO18,
            peripherals.GPIO23,
            peripherals.GPIO19,
            Rate::from_mhz(4),
        );
        let slow_spi = create_spi(
            peripherals.SPI2,
            peripherals.GPIO14,
            peripherals.GPIO13,
            peripherals.GPIO27, //GPIO12 is a bootstrapping pin and doin lotsa trouble on boot
            Rate::from_khz(400),
        );

        let fast_spi_ref_cell = RefCell::new(fast_spi);
        let slow_spi_ref_cell = RefCell::new(slow_spi);

        let mut buf512 = [0u8; 512];
        let display = init_ili9341_display(
            &fast_spi_ref_cell,
            peripherals.GPIO2,
            peripherals.GPIO15,
            peripherals.GPIO4,
            &mut buf512,
        )
        .unwrap();
        let mut slint_renderer = SlintRenderer::new(display);

        let window = self.window.borrow().clone().unwrap();
        window.set_size(PhysicalSize::new(320, 240));

        let mut touch_input = Xpt2046TouchInput::create(
            &fast_spi_ref_cell,
            peripherals.GPIO33,
            peripherals.GPIO36,
            320,
        )
        .unwrap();
        touch_input.init().unwrap();
        init_sd_card(&slow_spi_ref_cell, peripherals.GPIO21);
        loop {
            update_timers_and_animations();
            handle_input(&window, &mut touch_input)?;

            window.draw_if_needed(|renderer| {
                renderer.render_by_line(&mut slint_renderer);
            });
            window.request_redraw();
        }
    }
}

#[main]
fn main() -> ! {
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_println::logger::init_logger_from_env();

    slint::platform::set_platform(Box::new(EspBackend {
        peripherals: RefCell::new(Some(peripherals)),
        window: RefCell::new(None),
    }))
    .expect("backend already initialized");

    let app = MainWindow::new().unwrap();

    app.run().unwrap();

    loop {}
}
