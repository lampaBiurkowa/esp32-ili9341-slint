use alloc::string::{String, ToString};
use core::cell::RefCell;
use embedded_hal_bus::spi::{NoDelay, RefCellDevice};
use esp_hal::{
    Blocking,
    delay::Delay,
    gpio::{Input, InputPin, Level, Output, OutputPin},
    spi::master::Spi,
};
use thiserror::Error;
use xpt2046::Xpt2046;

#[derive(Error, Debug)]
pub(crate) enum TouchInputError {
    #[error("Failed to initialize Xpt2046 driver")]
    Xpt2046Init,
    #[error("Failed to initialize SPI device for Xpt2046: {0}")]
    SpiInit(String),
    #[error("Failed to acquire input data")]
    AcquireInputData,
}

pub(crate) enum TouchInputResponse {
    Moved { x: i32, y: i32 },
    Pressed { x: i32, y: i32 },
    Released { x: i32, y: i32 },
    NoInput,
}

pub(crate) trait TouchInputProvider {
    fn get_input(&mut self) -> Result<TouchInputResponse, TouchInputError>;
}

pub(crate) struct Xpt2046TouchInput<'a> {
    driver: Xpt2046<RefCellDevice<'a, Spi<'a, Blocking>, Output<'a>, NoDelay>, Input<'a>>,
    last_pos: Option<(i32, i32)>,
    screen_width: i32,
}

impl<'a> Xpt2046TouchInput<'a> {
    pub(crate) fn create(
        spi: &'a RefCell<Spi<'a, Blocking>>,
        touch_cs_pin: impl OutputPin + 'a,
        irq_pin: impl InputPin + 'a,
        screen_width: i32,
    ) -> Result<Self, TouchInputError> {
        let touch_irq_pin = Input::new(irq_pin, Default::default());
        let touch_cs = Output::new(touch_cs_pin, Level::High, Default::default());
        let touch_spi_dev = RefCellDevice::new_no_delay(spi, touch_cs)
            .map_err(|e| TouchInputError::SpiInit(e.to_string()))?;
        let xpt = Xpt2046::new(
            touch_spi_dev,
            touch_irq_pin,
            xpt2046::Orientation::Landscape,
        );
        Ok(Self {
            driver: xpt,
            last_pos: None,
            screen_width,
        })
    }

    pub(crate) fn init(&mut self) -> Result<(), TouchInputError> {
        self.driver
            .init(&mut Delay::new())
            .map_err(|_| TouchInputError::Xpt2046Init)
    }
}

impl<'a> TouchInputProvider for Xpt2046TouchInput<'a> {
    fn get_input(&mut self) -> Result<TouchInputResponse, TouchInputError> {
        self.driver
            .run()
            .map_err(|_| TouchInputError::AcquireInputData)?;

        if self.driver.is_touched() {
            let p = self.driver.get_touch_point();
            let x = self.screen_width - 2 * p.x; //awkward adjustments but match my screen orientation
            let y = 2 * p.y;

            match self.last_pos.replace((x, y)) {
                Some(prev) if (prev.0 != x && prev.1 != y) => {
                    Ok(TouchInputResponse::Moved { x, y })
                }
                None => Ok(TouchInputResponse::Pressed { x, y }),
                _ => Ok(TouchInputResponse::Moved { x, y }),
            }
        } else if let Some(prev) = self.last_pos.take() {
            Ok(TouchInputResponse::Released {
                x: prev.0,
                y: prev.1,
            })
        } else {
            Ok(TouchInputResponse::NoInput)
        }
    }
}
