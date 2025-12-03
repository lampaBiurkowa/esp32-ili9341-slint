#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use alloc::{boxed::Box, rc::Rc};
use core::{cell::RefCell, ops::Range};
use embedded_graphics_core::pixelcolor::{Rgb565, raw::RawU16};
use embedded_hal_bus::spi::{NoDelay, RefCellDevice};
use esp_backtrace as _;
use esp_hal::{
    Blocking,
    delay::Delay,
    gpio::{
        Input, Level, Output,
        interconnect::{PeripheralInput, PeripheralOutput},
    },
    main,
    peripherals::Peripherals,
    spi::master::Spi,
    time::{Instant, Rate},
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
use embedded_sdmmc::{Mode, SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

slint::include_modules!();

struct DrawBuf<'a> {
    display: Display<
        SpiInterface<
            'a,
            RefCellDevice<
                'a,
                Spi<'a, Blocking>,
                Output<'a>,
                NoDelay,
            >,
            Output<'a>,
        >,
        ILI9341Rgb565,
        Output<'a>,
    >,
    buffer: [Rgb565Pixel; 320],
}

impl<'a> DrawBuf<'a> {
    fn new(
        spi: &'a RefCell<Spi<'a, Blocking>>,
        dc_pin: impl esp_hal::gpio::OutputPin + 'a,
        cs_pin: impl esp_hal::gpio::OutputPin + 'a,
        rst_pin: impl esp_hal::gpio::OutputPin + 'a,
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
        touch_cs_pin: impl esp_hal::gpio::OutputPin + 'a,
        irq_pin: impl esp_hal::gpio::InputPin + 'a,
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
    sd_cs_pin: impl esp_hal::gpio::OutputPin + 'a,
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
        //SD requires 100kHz-400kHz 
        //Display in order to be fast needs like 40MHz
        //XPT 2046 can have around 4MHz - it doesn't work on values that are too big
        let fast_spi = create_spi(
            peripherals.SPI3,
            peripherals.GPIO18,
            peripherals.GPIO23,
            peripherals.GPIO19,
            Rate::from_mhz(4)
        );
        let slow_spi = create_spi(
            peripherals.SPI2,
            peripherals.GPIO14,
            peripherals.GPIO13,
            peripherals.GPIO27, //GPIO12 is a bootstrapping pin and doin lotsa trouble on boot
            Rate::from_khz(400)
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

        // let mut uart = Uart::new(peripherals.UART0, Default::default()).unwrap();
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

    let config = esp_hal::Config::default();
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
