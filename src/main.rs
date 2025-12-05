#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use alloc::{boxed::Box, format, rc::Rc};
use blocking_network_stack::Stack;
use esp_println::println;
use esp_radio::wifi::{ClientConfig, ModeConfig, ScanConfig, WifiController};
use smoltcp::{iface::{SocketSet, SocketStorage}, wire::DhcpOption};
use core::{cell::RefCell, ops::Range};
use embedded_graphics_core::pixelcolor::{Rgb565, raw::RawU16};
use embedded_hal_bus::spi::{NoDelay, RefCellDevice};
use embedded_sdmmc::{Mode, SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use esp_backtrace as _;
use esp_hal::{
    Blocking, clock::CpuClock, delay::Delay, gpio::{
        Input, InputPin, Level, Output, OutputPin, interconnect::{PeripheralInput, PeripheralOutput}
    }, main, peripherals::Peripherals, rng::Rng, spi::master::Spi, time::{Duration, Instant, Rate}, timer::timg::TimerGroup
};
use mipidsi::{
    Display,
    interface::SpiInterface,
    models::ILI9341Rgb565,
    options::{ColorOrder, Orientation, Rotation},
};
use slint::{
    PhysicalPosition, PhysicalSize, PlatformError,
    platform::{
        Platform, PointerEventButton, WindowAdapter, WindowEvent,
        software_renderer::{
            LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel,
        },
        update_timers_and_animations,
    },
};
use xpt2046::Xpt2046;
use embedded_io::{Read, Write};

use crate::secrets::{TEST_ADDRESS, TEST_IP, WIFI_PASSWORD, WIFI_SSID};

extern crate alloc;

mod secrets;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

slint::include_modules!();

struct DrawBuf<'a> {
    display: Display<
        SpiInterface<'a, RefCellDevice<'a, Spi<'a, Blocking>, Output<'a>, NoDelay>, Output<'a>>,
        ILI9341Rgb565,
        Output<'a>,
    >,
    buffer: [Rgb565Pixel; 320],
}

impl<'a> DrawBuf<'a> {
    fn new(
        spi: &'a RefCell<Spi<'a, Blocking>>,
        dc_pin: impl OutputPin + 'a,
        cs_pin: impl OutputPin + 'a,
        rst_pin: impl OutputPin + 'a,
        buf512: &'a mut [u8; 512],
    ) -> Self {
        let dc = Output::new(dc_pin, Level::Low, Default::default());
        let cs = Output::new(cs_pin, Level::Low, Default::default());
        let rst = Output::new(rst_pin, Level::Low, Default::default());
        let spi = RefCellDevice::new_no_delay(spi, cs).unwrap();
        let interface = SpiInterface::new(spi, dc, buf512);

        let display: Display<
            SpiInterface<'_, RefCellDevice<Spi<'_, Blocking>, Output<'_>, NoDelay>, Output<'_>>,
            ILI9341Rgb565,
            Output<'_>,
        > = mipidsi::Builder::new(ILI9341Rgb565, interface)
            .reset_pin(rst)
            .orientation(Orientation::new().rotate(Rotation::Deg270).flip_vertical())
            .color_order(ColorOrder::Bgr)
            .init(&mut Delay::new())
            .unwrap();

        let linebuf = [Rgb565Pixel(0); 320];
        Self {
            display,
            buffer: linebuf,
        }
    }
}

impl LineBufferProvider for &mut DrawBuf<'_> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: Range<usize>,
        render_fn: impl FnOnce(&mut [Rgb565Pixel]),
    ) {
        let buf = &mut self.buffer[range.clone()];
        render_fn(buf);
        self.display
            .set_pixels(
                range.start as u16,
                line as u16,
                range.end as u16,
                line as u16,
                buf.iter().map(|x| Rgb565::from(RawU16::new(x.0))),
            )
            .unwrap();
    }
}

struct Touch<'a> {
    xpt: Xpt2046<RefCellDevice<'a, Spi<'a, Blocking>, Output<'a>, NoDelay>, Input<'a>>,
    last_pos: Option<slint::PhysicalPosition>,
}

impl<'a> Touch<'a> {
    fn new(
        spi: &'a RefCell<Spi<'a, Blocking>>,
        touch_cs_pin: impl OutputPin + 'a,
        irq_pin: impl InputPin + 'a,
    ) -> Self {
        let touch_irq_pin = Input::new(irq_pin, Default::default());
        let touch_cs = Output::new(touch_cs_pin, Level::High, Default::default());
        let touch_spi_dev = RefCellDevice::new_no_delay(spi, touch_cs).unwrap();
        let mut xpt = Xpt2046::new(
            touch_spi_dev,
            touch_irq_pin,
            xpt2046::Orientation::Landscape,
        );
        xpt.init(&mut Delay::new()).unwrap();
        Self {
            xpt,
            last_pos: None,
        }
    }

    fn update(
        &mut self,
        window: &Rc<MinimalSoftwareWindow>,
        screen_w: i32,
        _screen_h: i32,
    ) -> Result<(), PlatformError> {
        self.xpt.run().unwrap();

        if self.xpt.is_touched() {
            let p = self.xpt.get_touch_point();
            let x_px = screen_w - 2 * p.x;
            let y_px = 2 * p.y;

            let pos = PhysicalPosition::new(x_px, y_px);
            let logical = pos.to_logical(window.scale_factor());

            let event = match self.last_pos.replace(pos) {
                Some(prev) if prev != pos => WindowEvent::PointerMoved { position: logical },
                None => WindowEvent::PointerPressed {
                    position: logical,
                    button: PointerEventButton::Left,
                },
                _ => WindowEvent::PointerMoved { position: logical },
            };

            window.try_dispatch_event(event)?;
        } else if let Some(prev) = self.last_pos.take() {
            window.try_dispatch_event(WindowEvent::PointerReleased {
                position: prev.to_logical(window.scale_factor()),
                button: PointerEventButton::Left,
            })?;
            window.try_dispatch_event(WindowEvent::PointerExited)?;
        }

        Ok(())
    }
}

struct DummyTime;
impl TimeSource for DummyTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp::from_calendar(2024, 1, 1, 0, 0, 0).unwrap()
    }
}

fn init_sd_card<'a>(
    spi: &'a RefCell<Spi<'a, Blocking>>,
    sd_cs_pin: impl OutputPin + 'a,
) {
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
    let request = format!("GET /api/Tags/tag-crime HTTP/1.1\r\n\
                    Host: {TEST_ADDRESS}\r\n\
                    Connection: close\r\n\
                    User-Agent: esp32-rust\r\n\
                    \r\n");
    socket
        .write(request.as_bytes())
        .unwrap();
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

        // let mut stack = setup_network_stack(device, &mut rng);
        let mut socket_set_entries: [SocketStorage; 3] = Default::default();
        let mut socket_set = SocketSet::new(&mut socket_set_entries[..]);
        let mut dhcp_socket = smoltcp::socket::dhcpv4::Socket::new();

        // we can set a hostname here (or add other DHCP options)
        dhcp_socket.set_outgoing_options(&[DhcpOption {
            kind: 12,
            data: b"implRust",
        }]);
        socket_set.add(dhcp_socket);
        // sta_socket_set.add(smoltcp::socket::dhcpv4::Socket::new());

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
        let mut drawbuf = DrawBuf::new(
            &fast_spi_ref_cell,
            peripherals.GPIO2,
            peripherals.GPIO15,
            peripherals.GPIO4,
            &mut buf512,
        );

        let window = self.window.borrow().clone().unwrap();
        window.set_size(PhysicalSize::new(320, 240));

        let mut xpt = Touch::new(&fast_spi_ref_cell, peripherals.GPIO33, peripherals.GPIO36);
        init_sd_card(&slow_spi_ref_cell, peripherals.GPIO21);
        loop {
            update_timers_and_animations();
            xpt.update(&window, 320, 240).unwrap();

            window.draw_if_needed(|renderer| {
                renderer.render_by_line(&mut drawbuf);
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
