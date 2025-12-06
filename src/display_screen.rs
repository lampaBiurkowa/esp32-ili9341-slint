use core::cell::RefCell;

use alloc::string::{String, ToString};
use embedded_hal_bus::spi::{NoDelay, RefCellDevice};
use esp_hal::{
    Blocking,
    delay::Delay,
    gpio::{Level, Output, OutputPin},
    spi::master::Spi,
};
use mipidsi::{
    Builder, Display,
    interface::SpiInterface,
    models::ILI9341Rgb565,
    options::{ColorOrder, Orientation, Rotation},
};
use thiserror::Error;

#[derive(Error, Debug)]
pub(crate) enum DisplayScreenError {
    #[error("Failed to initialize Ili9341 driver")]
    Ili9341Init,
    #[error("Failed to initialize SPI device for Xpt2046: {0}")]
    SpiInit(String),
}

pub(crate) fn init_ili9341_display<'a>(
    spi: &'a RefCell<Spi<'a, Blocking>>,
    dc_pin: impl OutputPin + 'a,
    cs_pin: impl OutputPin + 'a,
    rst_pin: impl OutputPin + 'a,
    buf512: &'a mut [u8; 512],
) -> Result<
    Display<
        SpiInterface<'a, RefCellDevice<'a, Spi<'a, Blocking>, Output<'a>, NoDelay>, Output<'a>>,
        ILI9341Rgb565,
        Output<'a>,
    >,
    DisplayScreenError,
> {
    let dc = Output::new(dc_pin, Level::Low, Default::default());
    let cs = Output::new(cs_pin, Level::Low, Default::default());
    let rst = Output::new(rst_pin, Level::Low, Default::default());
    let spi = RefCellDevice::new_no_delay(spi, cs)
        .map_err(|e| DisplayScreenError::SpiInit(e.to_string()))?;
    let interface = SpiInterface::new(spi, dc, buf512);

    Builder::new(ILI9341Rgb565, interface)
        .reset_pin(rst)
        .orientation(Orientation::new().rotate(Rotation::Deg270).flip_vertical())
        .color_order(ColorOrder::Bgr)
        .init(&mut Delay::new())
        .map_err(|_| DisplayScreenError::Ili9341Init)
}
