#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
use core::fmt::Alignment;
use core::pin::Pin;

use alloc::string::ToString;

use defmt::info;
use embassy_executor::Spawner;
use embassy_net::driver::Driver;
use embassy_time::{Duration, Timer};
use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::mono_font::{ascii::FONT_6X10, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::primitives::{PrimitiveStyleBuilder, Rectangle};
use embedded_graphics::text::{Baseline, TextStyleBuilder};
use esp_hal::gpio::OutputPin;
use esp_hal::i2c::master::{AnyI2c, Config, I2c};
use esp_hal::ledc::timer;
use esp_hal::spi::master::Config as SConfig;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{clock, peripherals, rng, spi, Async, Blocking, DriverMode};
use esp_hal::{clock::CpuClock, gpio::OutputConfig};
use ieee80211::{match_frames, mgmt_frame::BeaconFrame};
use mipidsi::options::Orientation;
use mipidsi::Display;
use panic_rtt_target as _;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Primitive, PrimitiveStyle, Triangle},
    text::Text,
};
use esp_hal::{
    clock::CpuClock, interrupt::software::SoftwareInterruptControl, timer::timg::TimerGroup,
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

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    // generator version: 0.5.0

    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 64 * 1024);
    // COEX needs more RAM - so we've added some more
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0);

    // We must initialize some kind of interface and start it.
    let (_controller, interfaces) =
        esp_radio::wifi::new(peripherals.WIFI, Default::default()).unwrap();

    let mut sniffer = interfaces.sniffer;
    sniffer.set_promiscuous_mode(true).unwrap();
    sniffer.set_receive_cb(|packet| {
        let _ = match_frames! {
            packet.data,
            beacon = BeaconFrame => {
                let Some(ssid) = beacon.ssid() else {
                    return;
                };
                if critical_section::with(|cs| {
                    KNOWN_SSIDS.borrow_ref_mut(cs).insert(ssid.to_string())
                }) {
                    println!("Found new AP with SSID: {ssid}");
                }
            }
        };
    });

    // // find more examples https://github.com/embassy-rs/trouble/tree/main/examples/esp32
    // let transport = BleConnector::new(&wifi_init, peripherals.BT);
    // let rng = esp_hal::rng::Rng::new(peripherals.RNG);
    // let _ble_controller = ExternalController::<_, 20>::new(transport);
    // let mut buzzer = Output::new(peripherals.GPIO42, Level::Low, OutputConfig::default());
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
    let mut spi = Spi::new(peripherals.SPI2, SConfig::default())
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

    let style = PrimitiveStyleBuilder::new()
        .stroke_color(Rgb565::RED)
        .stroke_width(3)
        .fill_color(Rgb565::GREEN)
        .build();

    info!("Hello world!");
    Rectangle::new(Point::new(30, 20), Size::new(10, 15))
        .into_styled(style)
        .draw(&mut display)
        .unwrap();
    let style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    let clockd = I2c::new(peripherals.I2C0, Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO11)
        .with_scl(peripherals.GPIO10)
        .into_async();
    let mut clock = pcf85063a::PCF85063::new(clockd);
    clock.perform_software_reset().await.unwrap();
    clock.start_clock().await.unwrap();
    let mut prev = [' '; 8]; // "HH:MM:SS"
    loop {
        let hour = clock.get_datetime().await.unwrap().to_string();

        info!("hour: {}", &hour.as_str());

        let dt = clock.get_datetime().await.unwrap();

        let mut now = [' '; 8];

        now[0] = char::from_digit((dt.hour() / 10) as u32, 10).unwrap();
        now[1] = char::from_digit((dt.hour() % 10) as u32, 10).unwrap();
        now[2] = ':';
        now[3] = char::from_digit((dt.minute() / 10) as u32, 10).unwrap();
        now[4] = char::from_digit((dt.minute() % 10) as u32, 10).unwrap();
        now[5] = ':';
        now[6] = char::from_digit((dt.second() / 10) as u32, 10).unwrap();
        now[7] = char::from_digit((dt.second() % 10) as u32, 10).unwrap();
        let char_width = 12;

        for i in 0..8 {
            if now[i] != prev[i] {
                let x = 60 + i as i32 * char_width;

                Rectangle::new(Point::new(x, 130), Size::new(12, 22))
                    .into_styled(
                        PrimitiveStyleBuilder::new()
                            .fill_color(Rgb565::new(200, 200, 200))
                            .build(),
                    )
                    .draw(&mut display)
                    .unwrap();

                let mut buf = [0u8; 1];
                buf[0] = now[i] as u8;

                Text::new(
                    core::str::from_utf8(&buf).unwrap(),
                    Point::new(x, 150),
                    style,
                )
                .draw(&mut display)
                .unwrap();

                prev[i] = now[i];
            }
        }

        Timer::after_secs(1).await;
    }

    // Create a text at position (20, 30) and draw it using the previously defined style

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0-rc.0/examples/src/bin
}
