#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use core::ops::Range;

use esp_hal::clock::CpuClock;
use embedded_hal::digital::OutputPin;
use embedded_graphics_core::pixelcolor::raw::RawU16;
use embedded_graphics_core::pixelcolor::Rgb565;
use esp_hal::main;
use esp_hal::time::{Duration, Instant};
use esp_hal::uart::Uart;
use mipidsi::models::ILI9341Rgb565;
use slint::platform::software_renderer::{LineBufferProvider, MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

slint::slint! {
    export component MainWindow inherits Window {
        width: 240px;
        height: 320px;

        Text {
            text: "Hello Slint!";
            horizontal-alignment: center;
            vertical-alignment: center;
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

#[main]
fn main() -> ! {
    // generator version: 1.0.1
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);
    let mut uart = Uart::new(peripherals.UART0, esp_hal::uart::Config::default()).unwrap();
    uart.write("Hello world!1\n".as_bytes()).unwrap();

    let mut window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    uart.write("Hello2\n".as_bytes()).unwrap();
    window.set_size(slint::PhysicalSize::new(240, 320));
    uart.write("Hello3\n".as_bytes()).unwrap();

    loop {
        slint::platform::update_timers_and_animations();
    uart.write("Hello5\n".as_bytes()).unwrap();
        let delay_start = Instant::now();
        while delay_start.elapsed() < Duration::from_millis(500) {}
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0/examples/src/bin
}
