#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use core::cell::RefCell;
use core::ops::Range;

use embedded_graphics_core::prelude::RgbColor;
use esp_backtrace as _;
use alloc::boxed::Box;
use alloc::rc::Rc;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::clock::CpuClock;
use embedded_hal::digital::OutputPin;
use embedded_graphics_core::pixelcolor::raw::RawU16;
use embedded_graphics_core::pixelcolor::Rgb565;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output};
use esp_hal::main;
use esp_hal::peripherals::Peripherals;
use esp_hal::spi::master::Spi;
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::uart::Uart;
use ili9341::{DisplaySize240x320, Ili9341};
use log::info;
use mipidsi::Builder;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorOrder, Orientation, Rotation};
use slint::platform::Platform;
use slint::platform::software_renderer::{LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

slint::slint! {
    export component MainWindow inherits Window {
        width: 320px;
        height: 240px;

        Rectangle {
            x: 0px;
            y: 0px;
            width: parent.width;
            height: parent.height;
            background: #00FF00;
        }
    }
}

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
        let mut uart = Uart::new(peripherals.UART0, esp_hal::uart::Config::default()).unwrap();
    uart.write("Hello world!1\n".as_bytes()).unwrap();
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
        // let interface = mipidsi::interface::SpiInterface::new(
        //     embedded_hal_bus::spi::ExclusiveDevice::new_no_delay(spi, cs).unwrap(),
        //     dc,
        //     &mut buf512,
        // );
        let interface = display_interface_spi::SPIInterface::new(
            embedded_hal_bus::spi::ExclusiveDevice::new_no_delay(spi, cs).unwrap(),
            dc,
            // &mut buf512,
        );

        // let mut display = mipidsi::Builder::new(mipidsi::models::ILI9341Rgb565, interface)
        //     .reset_pin(rst)
        //     .orientation(Orientation::new().rotate(Rotation::Deg180))
        //     .color_order(ColorOrder::Bgr)
        //     .init(&mut esp_hal::delay::Delay::new())
        //     .unwrap();
        let mut display = Ili9341::new(
      interface,
      rst,
      &mut Delay::new(),
      ili9341::Orientation::Landscape,
      DisplaySize240x320,
  )
  .unwrap();

        // Create the draw buffer
        let mut linebuf = [Rgb565Pixel(0); 320];
        let mut drawbuf = DrawBuf {
            display,
            buffer: &mut linebuf,
        };

        // Get the Slint window that was created earlier
        let window = self.window.borrow().clone().unwrap();
        window.set_size(slint::PhysicalSize::new(240, 320));
        uart.write("Hello world!2\n".as_bytes()).unwrap();
        window.request_redraw();

let fb = [Rgb565::new(0,255,0); 10];   // just 10 pixels
use embedded_graphics_core::draw_target::DrawTarget;

        // Main loop
        loop {
            slint::platform::update_timers_and_animations();

            window.draw_if_needed(|renderer| {
                drawbuf.display.clear(Rgb565::GREEN).unwrap();
// drawbuf.display
//         .set_pixels(0, 0, 10, 1, fb)
//         .unwrap();
        uart.write("3".as_bytes()).unwrap();
                // renderer.render_by_line(&mut drawbuf);
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
