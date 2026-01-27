use core::ops::Range;

use embedded_graphics_core::pixelcolor::raw::RawU16;
use esp_hal::gpio::Output;
use mipidsi::{
    Display,
    interface::{Interface, InterfacePixelFormat},
    models::Model,
};
use slint::platform::software_renderer::{LineBufferProvider, Rgb565Pixel};

pub(crate) struct SlintRenderer<'a, DI: Interface, MODEL: Model>
where
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word> + From<RawU16>,
{
    display: Display<DI, MODEL, Output<'a>>,
    buffer: [Rgb565Pixel; 320],
}

impl<'a, DI: Interface, MODEL: Model> SlintRenderer<'a, DI, MODEL>
where
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word> + From<RawU16>,
{
    pub(crate) fn new(display: Display<DI, MODEL, Output<'a>>) -> Self {
        Self {
            display,
            buffer: [Rgb565Pixel(0); 320],
        }
    }
}

impl<'a, DI: Interface, MODEL: Model> LineBufferProvider for &mut SlintRenderer<'a, DI, MODEL>
where
    MODEL::ColorFormat: InterfacePixelFormat<DI::Word> + From<RawU16>,
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
                buf.iter()
                    .map(|x| MODEL::ColorFormat::from(RawU16::new(x.0))),
            )
            .unwrap();
    }
}
