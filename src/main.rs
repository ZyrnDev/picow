#![no_std]
#![no_main]
#![feature(impl_trait_in_assoc_type)]

use core::{future::Future, net::Ipv4Addr, sync::atomic::AtomicU16};

use cyw43::JoinOptions;
use cyw43_pio::PioSpi;
use embassy_net::Ipv4Cidr;
use embassy_rp::{
    adc::Async, gpio::{Level, Output}, peripherals::{DMA_CH0, PIO0}, pio::Pio
};

use defmt_rtt as _;
use embassy_time::Duration;
use panic_probe as _;
use picoserve::{make_static, routing::get, AppBuilder, AppRouter};
use rand::Rng;
// use defmt::*;
use core::fmt::Write;

embassy_rp::bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<embassy_rp::peripherals::PIO0>;
    ADC_IRQ_FIFO => embassy_rp::adc::InterruptHandler;
});

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(mut stack: embassy_net::Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

struct AppProps;

impl AppBuilder for AppProps {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> {
        picoserve::Router::new()
            .route("/", get(|| async move { "Hello World" }))
            .route("/metrics", get(|| async { Prometheus }))
    }
}

const WEB_TASK_POOL_SIZE: usize = 8;

#[embassy_executor::task(pool_size = WEB_TASK_POOL_SIZE)]
async fn web_task(
    id: usize,
    stack: embassy_net::Stack<'static>,
    app: &'static AppRouter<AppProps>,
    config: &'static picoserve::Config<Duration>,
) -> ! {
    let port = 80;
    let mut tcp_rx_buffer = [0; 1024];
    let mut tcp_tx_buffer = [0; 1024];
    let mut http_buffer = [0; 2048];

    picoserve::listen_and_serve(
        id,
        app,
        config,
        stack,
        port,
        &mut tcp_rx_buffer,
        &mut tcp_tx_buffer,
        &mut http_buffer,
    )
    .await
}

// ADC value
static ADC_VALUE: AtomicU16 = AtomicU16::new(0);

#[embassy_executor::task]
async fn read_sensor(mut channel: embassy_rp::adc::Channel<'static>, mut adc: embassy_rp::adc::Adc<'static, Async>) -> ! {
    loop {
        // setup ADC for pin 31
        let result = adc.read(&mut channel).await;
        match result {
            Ok(value) => ADC_VALUE.store(value, core::sync::atomic::Ordering::Relaxed),
            Err(_) => defmt::warn!("ADC read error"),
        }
        
        embassy_time::Timer::after(Duration::from_secs(2)).await;
    }
}

struct Prometheus;

const HEADER: &str = "# HELP adc_value The value read from the ADC\n# TYPE adc_value gauge\n";
const METRICS: [&str; 1] = [
    "adc_value{sensor=\"MQ-2\"} ",
];

impl picoserve::response::Content for Prometheus {
    fn content_type(&self) -> &'static str {
        "text/plain; version=0.0.4"
    }

    fn content_length(&self) -> usize {
        HEADER.len() + METRICS.iter().map(|m| m.len()).sum::<usize>() + 5
    }

     fn write_content<W: embedded_io_async::Write>(self, mut writer: W) -> impl Future<Output = Result<(), W::Error>> {
        async move {
            writer.write_all(HEADER.as_bytes()).await?;
            for metric in METRICS {
                writer.write_all(metric.as_bytes()).await?;
            }

            let adc_value = ADC_VALUE.load(core::sync::atomic::Ordering::Relaxed);
            let mut value = heapless::String::<32>::new();
            write!(value, "{:05}", adc_value).unwrap();

            writer.write_all(value.as_bytes()).await?;

            Ok(())
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: embassy_executor::Spawner) {
    let p = embassy_rp::init(Default::default());

    let channel = embassy_rp::adc::Channel::new_pin(p.PIN_26, embassy_rp::gpio::Pull::None);
    let adc = embassy_rp::adc::Adc::new(p.ADC, Irqs, embassy_rp::adc::Config::default());
    spawner.must_spawn(read_sensor(channel, adc));

    let fw = include_bytes!("../../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../../cyw43-firmware/43439A0_clm.bin");

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = cyw43_pio::PioSpi::new(
        &mut pio.common,
        pio.sm0,
        cyw43_pio::DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    let state = make_static!(cyw43::State, cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    spawner.must_spawn(wifi_task(runner));

    control.init(clm).await;
    control.set_power_management(cyw43::PowerManagementMode::None).await;

    let config = embassy_net::Config::ipv4_static(embassy_net::StaticConfigV4 {
       address: Ipv4Cidr::new(Ipv4Addr::new(192, 168, 1, 244), 24),
       dns_servers: heapless::Vec::new(),
       gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
    });
    let (stack, runner) = embassy_net::new(
        net_device,
        config,
        make_static!(
            embassy_net::StackResources::<WEB_TASK_POOL_SIZE>,
            embassy_net::StackResources::new()
        ),
        embassy_rp::clocks::RoscRng.gen(),
    );

    spawner.must_spawn(net_task(runner));


    
    loop {
        match control.join(core::option_env!("WIFI_NETWORK").unwrap(), JoinOptions::new(core::option_env!("WIFI_PASSWORD").unwrap().as_bytes())).await {
            Ok(_) => {
                defmt::info!("Connected to WiFi");
                break;
            }
            Err(e) => {
                defmt::error!("Failed to connect to WiFi: STATUS = {:?}", e.status);
                embassy_time::Timer::after(Duration::from_secs(1)).await;
            }
            
        }
    }
    
    while !stack.is_config_up() {
        defmt::info!("Waiting for DHCP configuration...");
        embassy_time::Timer::after(Duration::from_secs(1)).await;
    }

    let app = make_static!(AppRouter<AppProps>, AppProps.build_app());

    let config = make_static!(
        picoserve::Config::<Duration>,
        picoserve::Config::new(picoserve::Timeouts {
            start_read_request: Some(Duration::from_secs(5)),
            persistent_start_read_request: Some(Duration::from_secs(1)),
            read_request: Some(Duration::from_secs(1)),
            write: Some(Duration::from_secs(1)),
        })
        .keep_connection_alive()
    );

    defmt::info!("Starting web server on port 80");

    for id in 0..WEB_TASK_POOL_SIZE {
        spawner.must_spawn(web_task(id, stack, app, config));
    }

    // Turn on the LED to indicate that the server is running
    control.gpio_set(0, true).await;

}
