#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use core::cell::RefCell;
use core::ops::Range;

use embedded_hal_bus::spi::{NoDelay, RefCellDevice};
use esp_backtrace as _;
use alloc::boxed::Box;
use alloc::rc::Rc;
use embedded_graphics_core::pixelcolor::raw::RawU16;
use embedded_graphics_core::pixelcolor::Rgb565;
use esp_hal::gpio::{Input, Level, Output};
use esp_hal::{Blocking, main};
use esp_hal::peripherals::Peripherals;
use esp_hal::spi::master::Spi;
use esp_hal::time::{Instant, Rate};
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorOrder, Orientation, Rotation};
use slint::platform::{Platform, PointerEventButton, WindowEvent};
use slint::platform::software_renderer::{LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use xpt2046::{Xpt2046};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

slint::include_modules!();

struct DrawBuf<'a> {
    display: mipidsi::Display<mipidsi::interface::SpiInterface<'a, embedded_hal_bus::spi::RefCellDevice<'a, Spi<'a, esp_hal::Blocking>, Output<'a>, embedded_hal_bus::spi::NoDelay>, Output<'a>>, ILI9341Rgb565, Output<'a>>,
    buffer: [Rgb565Pixel; 320],
}

impl<'a> DrawBuf<'a> {
    fn new<PinDc: esp_hal::gpio::OutputPin + 'a, PinCs: esp_hal::gpio::OutputPin + 'a, PinRst: esp_hal::gpio::OutputPin + 'a>(spi: &'a RefCell<Spi<'a, Blocking>>, dc_pin: PinDc, cs_pin: PinCs, rst_pin: PinRst, buf512: &'a mut[u8; 512]) -> Self {
        let dc = Output::new(dc_pin, Level::Low, Default::default());
        let cs = Output::new(cs_pin, Level::Low, Default::default());
        let rst = Output::new(rst_pin, Level::Low, Default::default());
        
        let spi = embedded_hal_bus::spi::RefCellDevice::new_no_delay(spi, cs).unwrap();

        let interface = mipidsi::interface::SpiInterface::new(
            spi,
            dc,
            buf512,
        );

        let display: mipidsi::Display<mipidsi::interface::SpiInterface<'_, embedded_hal_bus::spi::RefCellDevice<Spi<'_, esp_hal::Blocking>, Output<'_>, embedded_hal_bus::spi::NoDelay>, Output<'_>>, ILI9341Rgb565, Output<'_>> = mipidsi::Builder::new(mipidsi::models::ILI9341Rgb565, interface)
            .reset_pin(rst)
            .orientation(Orientation::new().rotate(Rotation::Deg270).flip_vertical())
            .color_order(ColorOrder::Bgr)
            .init(&mut esp_hal::delay::Delay::new())
            .unwrap();

        let linebuf = [Rgb565Pixel(0); 320];
        Self {
            display,
            buffer: linebuf
        }
    }
}

impl LineBufferProvider for &mut DrawBuf<'_>
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

struct Touch<'a> {
    xpt: Xpt2046<RefCellDevice<'a, Spi<'a, Blocking>, Output<'a>, NoDelay>, Input<'a>>,
    last_pos: Option<slint::PhysicalPosition>,
}

impl<'a> Touch<'a>
where
{
    fn new<PinTouch: esp_hal::gpio::OutputPin + 'a, PinIRQ2: esp_hal::gpio::InputPin + 'a>(spi: &'a RefCell<Spi<'a, Blocking>>, touch_cs_pin: PinTouch, irq_pin: PinIRQ2) -> Self {
        let touch_irq_pin = Input::new(irq_pin, Default::default());
        let touch_cs = Output::new(touch_cs_pin, Level::High, Default::default());
        let touch_spi_dev = embedded_hal_bus::spi::RefCellDevice::new_no_delay(spi, touch_cs).unwrap();
        let mut xpt = Xpt2046::new(touch_spi_dev, touch_irq_pin, xpt2046::Orientation::Landscape);
        xpt.init(&mut esp_hal::delay::Delay::new()).unwrap();
        Self {
            xpt,
            last_pos: None,
        }
    }

    fn update(
        &mut self,
        window: &Rc<MinimalSoftwareWindow>,
        screen_w: i32,
        screen_h: i32,
    ) -> Result<(), slint::PlatformError> {
        self.xpt.run().unwrap();

        if self.xpt.is_touched() {
            let p = self.xpt.get_touch_point();
            let x_px = screen_w - 2 * p.x;
            let y_px = 2 * p.y;

            let pos = slint::PhysicalPosition::new(x_px, y_px);
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
                .with_frequency(Rate::from_mhz(2))//40))
                .with_mode(esp_hal::spi::Mode::_0),
        )
        .unwrap()
        .with_sck(peripherals.GPIO18)
        .with_mosi(peripherals.GPIO23)
        .with_miso(peripherals.GPIO19);

        // // Display pins
        // let dc = Output::new(peripherals.GPIO2, Level::Low, Default::default());
        // let cs = Output::new(peripherals.GPIO15, Level::Low, Default::default());
        // let rst = Output::new(peripherals.GPIO4, Level::Low, Default::default());
        
        let spi_ref_cell = RefCell::new(spi);
        // let spi = embedded_hal_bus::spi::RefCellDevice::new_no_delay(&spi_ref_cell, cs).unwrap();

        let mut buf512 = [0u8; 512];
        // let interface = mipidsi::interface::SpiInterface::new(
        //     spi,
        //     dc,
        //     &mut buf512,
        // );

        // let display: mipidsi::Display<mipidsi::interface::SpiInterface<'_, embedded_hal_bus::spi::RefCellDevice<Spi<'_, esp_hal::Blocking>, Output<'_>, embedded_hal_bus::spi::NoDelay>, Output<'_>>, ILI9341Rgb565, Output<'_>> = mipidsi::Builder::new(mipidsi::models::ILI9341Rgb565, interface)
        //     .reset_pin(rst)
        //     .orientation(Orientation::new().rotate(Rotation::Deg270).flip_vertical())
        //     .color_order(ColorOrder::Bgr)
        //     .init(&mut esp_hal::delay::Delay::new())
        //     .unwrap();

        // // Create the draw buffer
        // let mut linebuf = [Rgb565Pixel(0); 320];
        let mut drawbuf = DrawBuf::new(&spi_ref_cell, peripherals.GPIO2, peripherals.GPIO15, peripherals.GPIO4, &mut buf512);
        //     display,
        //     buffer: &mut linebuf,
        // };

        // Get the Slint window that was created earlier
        let window = self.window.borrow().clone().unwrap();
        window.set_size(slint::PhysicalSize::new(320, 240));

        // let mut uart = Uart::new(peripherals.UART0, Default::default()).unwrap();
        let mut xpt = Touch::new(&spi_ref_cell, peripherals.GPIO33, peripherals.GPIO36);
        loop {
            slint::platform::update_timers_and_animations();
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
