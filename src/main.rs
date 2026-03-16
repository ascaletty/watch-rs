#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
use core::fmt::Alignment;
use core::pin::Pin;
use core::time::Duration;

use defmt::info;
use embassy_time::Timer;
use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::mono_font::{ascii::FONT_6X10, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::primitives::{PrimitiveStyleBuilder, Rectangle};
use embedded_graphics::text::{Baseline, TextStyleBuilder};
use esp_hal::i2c::master::{AnyI2c, Config, I2c};
use esp_hal::ledc::channel::config;
use esp_hal::spi::master::Config as SConfig;
use esp_hal::{clock::CpuClock, gpio::OutputConfig};
use esp_radio::wifi::{Interfaces, WifiDevice, WifiEvent};
use ieee80211::{match_frames, mgmt_frame::BeaconFrame};
use mipidsi::options::Orientation;
use panic_rtt_target as _;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Primitive, PrimitiveStyle, Triangle},
    text::Text,
};
use embedded_hal_bus::spi::{ExclusiveDevice, NoDelay};
use esp_hal::gpio::{Level, Output};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::spi::master::Spi;
use esp_hal::timer::AnyTimer;
// Provides the parallel port and display interface buildersk
use esp_radio::wifi::ScanConfig;
use mipidsi::interface::SpiInterface;
use mipidsi::{models::ST7789, Builder};
extern crate alloc;
use core::cell::RefCell;
use critical_section::Mutex;
// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();
use alloc::{
    collections::btree_set::BTreeSet,
    string::{String, ToString},
};
use esp_hal::delay::Delay;
use esp_radio::wifi::ClientConfig;
static KNOWN_SSIDS: Mutex<RefCell<BTreeSet<String>>> = Mutex::new(RefCell::new(BTreeSet::new()));

const SSID: &str = "Scaletty";
const PASSWORD: &str = "ALCS2014";
const NTP_SERVER: &str = "pool.ntp.org";
use esp_hal::timer::timg::TimerGroup;
use static_cell::StaticCell;
static WCONTROLLER: StaticCell<WifiController> = StaticCell::new();
static CONTROLLER: StaticCell<esp_radio::Controller> = StaticCell::new();
use core::net::{IpAddr, SocketAddr};
use defmt::error;
use embassy_net::{Config as NetConfig, DhcpConfig};
use esp_hal::rtc_cntl::Rtc;
use esp_radio::wifi::AuthMethod;
use sntpc::{get_time, NtpContext, NtpTimestampGenerator};
use sntpc_net_embassy::UdpSocketWrapper;
#[derive(Clone, Copy)]
struct Timestamp<'a> {
    rtc: &'a Rtc<'a>,
    current_time_us: u64,
}

impl NtpTimestampGenerator for Timestamp<'_> {
    fn init(&mut self) {
        self.current_time_us = self.rtc.current_time_us();
    }

    fn timestamp_sec(&self) -> u64 {
        self.current_time_us / 1_000_000
    }

    fn timestamp_subsec_micros(&self) -> u32 {
        (self.current_time_us % 1_000_000) as u32
    }
}
use esp_hal::rng::Rng;
#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    // generator version: 0.5.0
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    let rtc = Rtc::new(peripherals.LPWR);

    esp_alloc::heap_allocator!(size: 72 * 1024);
    // COEX needs more RAM - so we've added some more
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0);
    let station_config = ClientConfig::default()
        .with_ssid(SSID.to_string())
        .with_password(PASSWORD.into())
        .with_auth_method(AuthMethod::Wpa2Personal);

    let controller = CONTROLLER.init(esp_radio::init().unwrap());

    let (mut controller, interfaces) =
        esp_radio::wifi::new(controller, peripherals.WIFI, Default::default()).unwrap();

    controller
        .set_config(&esp_radio::wifi::ModeConfig::Client(station_config))
        .ok();
    let controller_ref = WCONTROLLER.init(controller);
    static RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();

    // Create TUN/TAP device

    // Configure network stack
    let rng = Rng::new();
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;
    let config = embassy_net::Config::dhcpv4(DhcpConfig::default());
    // Init network stack
    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    spawner.spawn(connection(controller_ref)).ok();

    spawner.spawn(net_task(runner)).ok();
    stack.wait_config_up().await;
    info!("IP config: {:?}", stack.config_v4());
    info!("network ready!!");
    Timer::after_secs(2).await;
    // let mut rx_meta = [PacketMetadata::EMPTY; 16];
    // let mut rx_buffer = [0; 4096];
    // let mut tx_meta = [PacketMetadata::EMPTY; 16];
    // let mut tx_buffer = [0; 4096];
    let mut rx_meta = [PacketMetadata::EMPTY; 32];
    let mut rx_buffer = [0; 8192];

    let mut tx_meta = [PacketMetadata::EMPTY; 32];
    let mut tx_buffer = [0; 8192];

    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    socket.bind(0).unwrap();
    info!("socket binded");
    let socket = UdpSocketWrapper::new(socket);

    let ntp_addrs = stack
        .dns_query(NTP_SERVER, DnsQueryType::A)
        .await
        .expect("Failed to resolve DNS");
    if ntp_addrs.is_empty() {
        error!("Failed to resolve DNS");
        return;
    }
    info!("dns dns_query");
    info!("NTP addr {:?}", ntp_addrs[0]);
    let server = SocketAddr::new(IpAddr::from(ntp_addrs[0]), 123);
    let ntp_time = get_time(
        server,
        &socket,
        NtpContext::new(Timestamp {
            rtc: &rtc,
            current_time_us: 0,
        }),
    )
    .await
    .unwrap();
    info!("packet left");
    let mut nowtime = jiff::Timestamp::from_second(ntp_time.sec() as i64).unwrap();
    let mut zdt = nowtime.to_zoned(jiff::tz::TimeZone::UTC);
    zdt -= jiff::Span::new().hours(5);

    let hour = zdt.hour();
    let minute = zdt.minute();
    let second = zdt.second();

    let year = zdt.year();
    let month = zdt.month();
    let day = zdt.day();
    info!("Time is {}:{}", hour, minute);
    info!("date is {}/{}/{}", month, day, year);

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
    let dc = esp_hal::gpio::Output::new(
        peripherals.GPIO4,
        esp_hal::gpio::Level::Low,
        OutputConfig::default(),
    );
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

    let style = PrimitiveStyleBuilder::new()
        .stroke_color(Rgb565::RED)
        .stroke_width(3)
        .fill_color(Rgb565::GREEN)
        .build();

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
    let mut now = [' '; 8];
    loop {
        let char_width = 12;

        let mut prev = [' '; 8];

        prev[0] = char::from_digit((zdt.hour() / 10) as u32, 10).unwrap();
        prev[1] = char::from_digit((zdt.hour() % 10) as u32, 10).unwrap();
        prev[2] = ':';
        prev[3] = char::from_digit((zdt.minute() / 10) as u32, 10).unwrap();
        prev[4] = char::from_digit((zdt.minute() % 10) as u32, 10).unwrap();
        prev[5] = ':';
        prev[6] = char::from_digit((zdt.second() / 10) as u32, 10).unwrap();
        prev[7] = char::from_digit((zdt.second() % 10) as u32, 10).unwrap();
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
                buf[0] = prev[i] as u8;

                Text::new(
                    core::str::from_utf8(&buf).unwrap(),
                    Point::new(x, 150),
                    style,
                )
                .draw(&mut display)
                .unwrap();

                now[i] = prev[i];
            }
        }
        embassy_time::Timer::after_secs(1).await;
        zdt += jiff::Span::new().seconds(1);
    }

    // Create a text at position (20, 30) and draw it using the previously defined style

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.0.0-rc.0/examples/src/bin
}
#[embassy_executor::task]
async fn sniffer(interfaces: Interfaces<'static>) {
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
                    info!("Found new AP with SSID: {}", ssid);
                }
            }
        };
    });
}
use esp_radio::wifi::WifiController;
#[embassy_executor::task]
async fn connection(controller: &'static mut WifiController<'static>) {
    info!("wifi task started");

    controller.start_async().await.unwrap();
    info!("wifi started");

    loop {
        info!("connecting...");

        match controller.connect_async().await {
            Ok(info) => {
                info!("connected {:?}", info);

                // Wait until WiFi disconnects
                controller.wait_for_event(WifiEvent::StaDisconnected).await;

                info!("wifi disconnected");
            }

            Err(e) => {
                info!("connect error: {:?}", e);
            }
        }

        // Prevent reconnect storms
        Timer::after_secs(3).await;
    }
}
use embassy_net::dns::DnsQueryType;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{Config as EConfig, Ipv4Address, Ipv4Cidr, StackResources};
use heapless::Vec;
#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, WifiDevice<'static>>) -> ! {
    runner.run().await
}
