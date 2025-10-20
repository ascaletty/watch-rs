#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
//! The following wiring is assumed:
//LCD DC GPIO4
//LCD CS GPIO5
//LCD SDL GPIO6
//LCD SDA GPIO7
//LCD RST GPIO8
//TP SCL ESP32_SCL
//TP SDA ESP32_SDA
//TP RST GPIO13
//TP INT GPIO14
//LCD BL GPIO15
//Buzz GPIO42
//BAT ADC GPIO1

use core::pin::Pin;

use bt_hci::cmd::info;
use bt_hci::controller::ExternalController;
use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::gpio::OutputPin;
use esp_hal::spi::master::Config;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{clock::CpuClock, gpio::OutputConfig};
use esp_hal::{peripherals, spi, Blocking, DriverMode};
use esp_wifi::ble::controller::BleConnector;
use mipidsi::options::Orientation;
use mipidsi::Display;
use panic_rtt_target as _;

use esp_hal::{
    delay::Delay,
    gpio::{Io, Level, Output},
    rtc_cntl::Rtc,
    spi::master::{AnySpi, Spi},
    timer::timg::TimerGroup as Spi_time,
};

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Primitive, PrimitiveStyle, Triangle},
};

// Provides the parallel port and display interface builders
use mipidsi::interface::SpiInterface;

use embedded_hal_bus::spi::{ExclusiveDevice, NoDelay};

// Provides the Display builder
use mipidsi::{models::ST7789, Builder};
extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    // generator version: 0.5.0

    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 64 * 1024);
    // COEX needs more RAM - so we've added some more
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);

    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    info!("Embassy initialized!");

    // let rng = esp_hal::rng::Rng::new(peripherals.RNG);
    // let timer1 = TimerGroup::new(peripherals.TIMG0);
    // let wifi_init =
    //     esp_wifi::init(timer1.timer0, rng).expect("Failed to initialize WIFI/BLE controller");
    // let (mut _wifi_controller, _interfaces) = esp_wifi::wifi::new(&wifi_init, peripherals.WIFI)
    //     .expect("Failed to initialize WIFI controller");
    // // find more examples https://github.com/embassy-rs/trouble/tree/main/examples/esp32
    // let transport = BleConnector::new(&wifi_init, peripherals.BT);
    // let _ble_controller = ExternalController::<_, 20>::new(transport);
    let mut buzzer = Output::new(peripherals.GPIO42, Level::Low, OutputConfig::default());
    //LCD DC GPIO4
    //LCD CS GPIO5
    //LCD SDL GPIO6
    //LCD SDA GPIO7
    //LCD RST GPIO8
    //TP SCL ESP32_SCL
    //TP SDA ESP32_SDA
    //TP RST GPIO13
    //TP INT GPIO14
    //LCD BL GPIO15
    //Buzz GPIO42
    //BAT ADC GPIO1
    let dc = Output::new(peripherals.GPIO4, Level::Low, OutputConfig::default());
    // Define the reset pin as digital outputs and make it high
    let mut rst = Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default());

    let cs_output = Output::new(peripherals.GPIO5, Level::High, OutputConfig::default());
    let mut lcd_backligt = Output::new(peripherals.GPIO15, Level::Low, OutputConfig::default());
    lcd_backligt.set_high();
    let mut delay = Delay::new();
    rst.set_high();
    let mut spi = Spi::new(peripherals.SPI2, Config::default())
        .unwrap()
        .with_mosi(peripherals.GPIO7)
        .with_sck(peripherals.GPIO6);
    let spi_device = ExclusiveDevice::new_no_delay(spi, cs_output).unwrap();

    let mut buffer = [0_u8; 512];

    let di = SpiInterface::new(spi_device, dc, &mut buffer);
    let mut display = Builder::new(ST7789, di)
        .display_size(240, 300)
        .reset_pin(rst)
        .init(&mut delay)
        .unwrap();
    display.set_orientation(Orientation::default()).unwrap();
    // display.sleep(&mut delay).unwrap();
    display.clear(Rgb565::new(200, 200, 200)).unwrap();
    info!("cleared display");
    spawner.spawn(wake()).ok();

    loop {
        info!("Hello world!");
        Timer::after(Duration::from_secs(1)).await;
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0-rc.0/examples/src/bin
}
#[embassy_executor::task]
async fn wake() {
    info!("waking watch");
}
