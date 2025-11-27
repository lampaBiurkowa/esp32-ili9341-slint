#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use core::cell::RefCell;
use core::ops::Range;

use esp_backtrace as _;
use alloc::boxed::Box;
use alloc::rc::Rc;
use embedded_hal::digital::OutputPin;
use embedded_graphics_core::pixelcolor::raw::RawU16;
use embedded_graphics_core::pixelcolor::Rgb565;
use esp_hal::gpio::{Level, Output};
use esp_hal::main;
use esp_hal::peripherals::Peripherals;
use esp_hal::spi::master::Spi;
use esp_hal::time::{Instant, Rate};
use log::info;
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorOrder, Orientation, Rotation};
use slint::platform::Platform;
use slint::platform::software_renderer::{LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

slint::include_modules!();

struct DrawBuf<'a, Display> {
    display: Display,
    buffer: &'a mut [Rgb565Pixel],
}

impl<DI, RST> LineBufferProvider for &mut DrawBuf<'_, mipidsi::Display<DI, ILI9341Rgb565, RST>>
where
    DI: mipidsi::interface::Interface<Word = u8>,
    RST: OutputPin<Error = core::convert::Infallible>,
{
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
        buf.iter().map(|x| Rgb565::from(RawU16::new(x.0)))
            )
            .unwrap();
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
            peripherals: RefCell::new(None)
        }
    }
}

impl Platform for EspBackend {
    fn duration_since_start(&self) -> core::time::Duration {
        core::time::Duration::from_millis(Instant::now().duration_since_epoch().as_millis())
    }

    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        let w = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        self.window.replace(Some(w.clone()));
        Ok(w)
    }

    fn run_event_loop(&self) -> Result<(), slint::PlatformError> {
        let peripherals = self.peripherals.borrow_mut().take().expect("Peripherals already taken");
        let spi = Spi::<esp_hal::Blocking>::new(
            peripherals.SPI2,
            esp_hal::spi::master::Config::default()
                .with_frequency(Rate::from_mhz(40))
                .with_mode(esp_hal::spi::Mode::_0),
        )
        .unwrap()
        .with_sck(peripherals.GPIO18)
        .with_mosi(peripherals.GPIO23);

        // Display pins
        let dc = Output::new(peripherals.GPIO2, Level::Low, Default::default());
        let cs = Output::new(peripherals.GPIO15, Level::Low, Default::default());
        let rst = Output::new(peripherals.GPIO4, Level::Low, Default::default());

        let mut buf512 = [0u8; 512];
        let interface = mipidsi::interface::SpiInterface::new(
            embedded_hal_bus::spi::ExclusiveDevice::new_no_delay(spi, cs).unwrap(),
            dc,
            &mut buf512,
        );

        let display = mipidsi::Builder::new(mipidsi::models::ILI9341Rgb565, interface)
            .reset_pin(rst)
            .orientation(Orientation::new().rotate(Rotation::Deg270).flip_vertical())
            .color_order(ColorOrder::Bgr)
            .init(&mut esp_hal::delay::Delay::new())
            .unwrap();

        // Create the draw buffer
        let mut linebuf = [Rgb565Pixel(0); 320];
        let mut drawbuf = DrawBuf {
            display,
            buffer: &mut linebuf,
        };

        // Get the Slint window that was created earlier
        let window = self.window.borrow().clone().unwrap();
        window.set_size(slint::PhysicalSize::new(320, 240));

        // Main loop
        loop {
            slint::platform::update_timers_and_animations();

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
    info!("Peripherals initialized");

    slint::platform::set_platform(Box::new(EspBackend {
        peripherals: RefCell::new(Some(peripherals)),
        window: RefCell::new(None),
    }))
        .expect("backend already initialized");

    let app = MainWindow::new().unwrap();

    app.run().unwrap();

    loop {}
}
