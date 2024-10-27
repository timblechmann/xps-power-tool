extern crate tokio;
use futures::stream::StreamExt;
use heapless::{String, Vec};
use log::*;
use std::fs::File;
use std::io::Write;
use upower_dbus::UPowerProxy;

async fn write_sysfs(
    path: heapless::String<128>,
    content: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path.as_str());

    let mut file = match file {
        Ok(file) => file,
        Err(e) => {
            warn!("Couldn't open file for writing: {} ({})", &path, e);
            return Err(e.into());
        }
    };

    let success = file.write_all(content);
    match success {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!("Couldn't write to file: {} ({})", &path, e);
            Err(Box::new(e))
        }
    }
}
async fn set_performance_bias(bias: u8) {
    use std::fmt::Write;

    let paths = (0..=15).map(|index| {
        let mut str = String::<128>::new();
        write!(
            str,
            "/sys/devices/system/cpu/cpu{}/power/energy_perf_bias",
            index
        )
        .unwrap();
        str
    });

    let mut payload_string = String::<16>::new();
    write!(payload_string, "{}", bias).unwrap();
    let fns: Vec<_, 16> = paths
        .map(|path| write_sysfs(path, payload_string.as_bytes()))
        .collect();

    for f in fns {
        let _ = f.await;
    }
}

async fn handle_battery_update(status: Option<bool>) {
    match status {
        None => {}
        Some(value) => {
            if value {
                set_performance_bias(0).await
            } else {
                set_performance_bias(15).await
            }
        }
    };
}

async fn run_async() -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let upower = UPowerProxy::new(&connection).await?;
    let battery_state = upower.on_battery().await;
    trace!("On Battery: {:?}", battery_state);
    handle_battery_update(battery_state.ok()).await;

    let mut stream = upower.receive_on_battery_changed().await;
    while let Some(event) = stream.next().await {
        let battery_state = event.get().await;
        trace!("On Battery: {:?}", battery_state);
        handle_battery_update(battery_state.ok()).await;
    }

    Ok(())
}

use fern::colors::{Color, ColoredLevelConfig};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let colors = ColoredLevelConfig::new().debug(Color::Magenta);

    fern::Dispatch::new()
        // Perform allocation-free log formatting
        .format(move |out, message, record| {
            out.finish(format_args!(
                "{} [{} {} {}] {} \x1B[0m",
                format_args!("\x1B[{}m", colors.get_color(&record.level()).to_fg_str()),
                humantime::format_rfc3339(std::time::SystemTime::now()),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(LevelFilter::Debug)
        .chain(std::io::stdout())
        .apply()?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let ret = rt.block_on(async {
        info!("Starting upower dbus monitor...");

        let dbus_monitor = tokio::spawn(run_async());

        trace!("Registering sigint ...");

        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C signal handler");

        trace!("Sigint ...");
        dbus_monitor.abort();

        let _result = dbus_monitor.await;
        trace!("Sigint ... {:#?}", _result);
        _result
    });

    match ret {
        Ok(_) => Ok(()),
        Err(e) => {
            if e.is_cancelled() {
                Ok(())
            } else {
                Err(e.into())
            }
        }
    }
}
