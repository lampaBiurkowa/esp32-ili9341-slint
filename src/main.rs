#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use alloc::{boxed::Box, format, rc::Rc};
use blocking_network_stack::Stack;
use core::cell::RefCell;
use embedded_hal_bus::spi::RefCellDevice;
use embedded_io::{Read, Write};
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
    time::{Duration, Instant, Rate},
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp_radio::wifi::{ClientConfig, ModeConfig, ScanConfig, WifiController};
use slint::{
    PhysicalPosition, PhysicalSize, PlatformError,
    platform::{
        Platform, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        update_timers_and_animations,
    },
};
use smoltcp::{
    iface::{SocketSet, SocketStorage},
    wire::DhcpOption,
};

use crate::{
    display_screen::init_ili9341_display,
    secrets::{TEST_ADDRESS, TEST_IP, WIFI_PASSWORD, WIFI_SSID},
    slint_renderer::SlintRenderer,
    touch_input::{TouchInputProvider, TouchInputResponse, Xpt2046TouchInput},
};

extern crate alloc;

mod display_screen;
mod secrets;
mod slint_renderer;
mod touch_input;

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

pub fn create_interface(device: &mut esp_radio::wifi::WifiDevice) -> smoltcp::iface::Interface {
    smoltcp::iface::Interface::new(
        smoltcp::iface::Config::new(smoltcp::wire::HardwareAddress::Ethernet(
            smoltcp::wire::EthernetAddress::from_bytes(&device.mac_address()),
        )),
        device,
        timestamp(),
    )
}

fn timestamp() -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros(
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros() as i64,
    )
}

fn configure_wifi(controller: &mut WifiController<'_>) {
    controller
        .set_power_saving(esp_radio::wifi::PowerSaveMode::None)
        .unwrap();

    let client_config = ModeConfig::Client(
        ClientConfig::default()
            .with_ssid(WIFI_SSID.into())
            .with_password(WIFI_PASSWORD.into()),
    );
    let res = controller.set_config(&client_config);
    println!("wifi_set_configuration returned {:?}", res);

    controller.start().unwrap();
    println!("is wifi started: {:?}", controller.is_started());
}

fn scan_wifi(controller: &mut WifiController<'_>) {
    println!("Start Wifi Scan");
    let scan_config = ScanConfig::default().with_max(10);
    let res = controller.scan_with_config(scan_config).unwrap();
    for ap in res {
        println!("{:?}", ap);
    }
}

fn connect_wifi(controller: &mut WifiController<'_>) {
    println!("{:?}", controller.capabilities());
    println!("wifi_connect {:?}", controller.connect());

    println!("Wait to get connected");
    loop {
        match controller.is_connected() {
            Ok(true) => break,
            Ok(false) => {}
            Err(err) => panic!("{:?}", err),
        }
    }
    println!("Connected: {:?}", controller.is_connected());
}

fn obtain_ip(stack: &mut blocking_network_stack::Stack<'_, esp_radio::wifi::WifiDevice<'_>>) {
    println!("Wait for IP address");
    loop {
        stack.work();
        if stack.is_iface_up() {
            println!("IP acquired: {:?}", stack.get_ip_info());
            break;
        }
    }
}

fn http_request(
    mut socket: blocking_network_stack::Socket<'_, '_, esp_radio::wifi::WifiDevice<'_>>,
) {
    println!("Starting HTTP client loop");
    let delay = Delay::new();
    println!("Making HTTP request");
    socket.work();

    let remote_addr = TEST_IP;
    socket.open(remote_addr, 80).unwrap();
    let request = format!(
        "GET /api/Tags/tag-crime HTTP/1.1\r\n\
                    Host: {TEST_ADDRESS}\r\n\
                    Connection: close\r\n\
                    User-Agent: esp32-rust\r\n\
                    \r\n"
    );
    socket.write(request.as_bytes()).unwrap();
    socket.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut buffer = [0u8; 512];
    while let Ok(len) = socket.read(&mut buffer) {
        let Ok(text) = core::str::from_utf8(&buffer[..len]) else {
            panic!("Invalid UTF-8 sequence encountered");
        };

        println!("{}", text);

        if Instant::now() > deadline {
            println!("Timeout");
            break;
        }
    }

    socket.disconnect();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        socket.work();
    }

    delay.delay_millis(1000);
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
        let radio_init = esp_radio::init().expect("Failed to initialize Wi-Fi/BLE controller");

        let (mut wifi_controller, interfaces) =
            esp_radio::wifi::new(&radio_init, peripherals.WIFI, Default::default())
                .expect("Failed to initialize Wi-Fi controller");

        let mut device = interfaces.sta;
        let mut socket_set_entries: [SocketStorage; 3] = Default::default();
        let mut socket_set = SocketSet::new(&mut socket_set_entries[..]);
        let mut dhcp_socket = smoltcp::socket::dhcpv4::Socket::new();

        dhcp_socket.set_outgoing_options(&[DhcpOption {
            kind: 12,
            data: b"implRust",
        }]);
        socket_set.add(dhcp_socket);

        let now = || Instant::now().duration_since_epoch().as_millis();
        let mut stack = Stack::new(
            create_interface(&mut device),
            device,
            socket_set,
            now,
            rng.random(),
        );

        configure_wifi(&mut wifi_controller);
        scan_wifi(&mut wifi_controller);
        connect_wifi(&mut wifi_controller);
        obtain_ip(&mut stack);

        let mut rx_buffer = [0u8; 1536];
        let mut tx_buffer = [0u8; 1536];
        let socket = stack.get_socket(&mut rx_buffer, &mut tx_buffer);

        http_request(socket);

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
