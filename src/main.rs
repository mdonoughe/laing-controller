mod mqtt;
mod settings;
mod timeout;
mod transfer;

use anyhow::anyhow;
use log::{debug, error, info};
use mqtt::{MqttHandle, State};
use settings::{load_settings, Settings};
use std::time::Duration;
use timeout::TimeoutPort;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::oneshot,
};
use tokio_modbus::{client::Context, prelude::*};
use tokio_serial::SerialStream;
use transfer::TransferPort;

use crate::mqtt::mqtt_loop;

static WAKE: [u16; 14] = [
    0x0000, 0x0000, 0x0009, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017, 0x0000,
    0x0000, 0x0000,
];
static IDLE: [u16; 14] = [
    0x0000, 0x0000, 0x0000, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017, 0x0000,
    0x0000, 0x0000,
];
static PRESET1: [[u16; 14]; 2] = [
    [
        0x0000, 0x0000, 0x0001, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
    [
        0x0000, 0x0000, 0x0001, 0x0001, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
];
static PRESET2: [[u16; 14]; 2] = [
    [
        0x0000, 0x0000, 0x0002, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
    [
        0x0000, 0x0000, 0x0002, 0x0002, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
];
static PRESET3: [[u16; 14]; 2] = [
    [
        0x0000, 0x0000, 0x0003, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
    [
        0x0000, 0x0000, 0x0003, 0x0003, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
];
static PRESET4: [[u16; 14]; 2] = [
    [
        0x0000, 0x0000, 0x0004, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
    [
        0x0000, 0x0000, 0x0004, 0x0004, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
];

fn decode_digit(value: u8) -> Option<u8> {
    match value & 0x7f {
        0b0111111 => Some(0),
        0b0000110 => Some(1),
        0b1011011 => Some(2),
        0b1001111 => Some(3),
        0b1100110 => Some(4),
        0b1101101 => Some(5),
        0b1111101 => Some(6),
        0b0000111 => Some(7),
        0b1111111 => Some(8),
        0b1101111 => Some(9),
        _ => {
            dbg!(value);
            None
        }
    }
}

fn decode(values: &[u16; 2]) -> Option<u16> {
    if values[0] & 0x8080 != 0x8000 || values[1] & 0xff80 != 0 {
        None
    } else {
        Some(
            decode_digit((values[0] & 0xff) as u8)? as u16
                + 10 * decode_digit((values[0] >> 8 & 0x7f) as u8)? as u16
                + 100 * decode_digit((values[1] & 0xff) as u8)? as u16,
        )
    }
}

async fn transmit(
    client: &mut Context,
    send: &[u16; 14],
    mqtt: &mut MqttHandle,
) -> anyhow::Result<Option<u16>> {
    let response = client
        .read_write_multiple_registers(0x9c4, 20, 0xa8c, &send[..])
        .await?;

    let height = decode((&response[0..2]).try_into().unwrap());
    if let Some(height) = height {
        mqtt.set_height(f32::from(height) / 10.0f32)?;
    }

    Ok(height)
}

async fn operate<T: AsyncRead + AsyncWrite + Send + 'static>(
    port: &mut TransferPort<T>,
    server_addr: Slave,
    command: Option<&[[u16; 14]; 2]>,
    mqtt: &mut MqttHandle,
) -> anyhow::Result<Option<u16>> {
    let mut client = rtu::connect_slave(port.take(), server_addr).await?;
    debug!("sending wake message");
    loop {
        // The controller often reacts to but fails to respond to the first message.
        // Keep trying until we get a response.
        match transmit(&mut client, &WAKE, mqtt).await {
            Ok(_) => {
                break;
            }
            Err(err) => {
                error!("Failed to wake controller (will retry): {:?}", err);
                client.disconnect().await?;
                client = rtu::connect_slave(port.take(), server_addr).await?;
            }
        }
    }
    debug!("sending idle");
    let mut last_height = transmit(&mut client, &IDLE, mqtt).await?;
    if let Some(command) = command {
        debug!("sending lead");
        last_height = transmit(&mut client, &command[0], mqtt).await?;
        let mut since_change = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            debug!("sending command");
            let res = transmit(&mut client, &command[1], mqtt).await?;
            if res == last_height {
                if since_change < 1 {
                    since_change += 1;
                } else {
                    break;
                }
            } else {
                last_height = res;
            }
        }
        debug!("sending idle");
        last_height = transmit(&mut client, &IDLE, mqtt).await?;
    }

    client.disconnect().await?;

    Ok(last_height)
}

#[cfg(windows)]
windows_service::define_windows_service!(ffi_service_main, service_main);

#[cfg(windows)]
fn service_main(_arguments: Vec<std::ffi::OsString>) {
    if let Err(err) = real_service_main() {
        error!("Service failed: {:?}", err);
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn real_service_main() -> anyhow::Result<()> {
    use std::sync::{Arc, Mutex};

    use windows_service::{
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceStatus, ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    };

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let status_handle = Arc::new(Mutex::new(Option::<ServiceStatusHandle>::None));

    let main = {
        let mut lock = status_handle.lock().unwrap();
        let status_handle = status_handle.clone();
        let mut stop_tx = Some(stop_tx);
        *lock = Some(
            service_control_handler::register("laing-controller", move |control_event| {
                match control_event {
                    ServiceControl::Shutdown | ServiceControl::Stop => {
                        if let Some(stop_tx) = stop_tx.take() {
                            match stop_tx.send(()) {
                                Ok(()) => {
                                    status_handle
                                        .lock()
                                        .unwrap()
                                        .unwrap()
                                        .set_service_status(ServiceStatus {
                                            controls_accepted: ServiceControlAccept::empty(),
                                            current_state:
                                                windows_service::service::ServiceState::StopPending,
                                            service_type: ServiceType::OWN_PROCESS,
                                            exit_code: ServiceExitCode::NO_ERROR,
                                            checkpoint: 0,
                                            process_id: Some(std::process::id()),
                                            // Give us some time to stop in case the desk is in motion.
                                            wait_hint: Duration::from_secs(10),
                                        })
                                        .unwrap();
                                    ServiceControlHandlerResult::NoError
                                }
                                Err(()) => {
                                    error!("Clean service stop failed");
                                    ServiceControlHandlerResult::NoError
                                }
                            }
                        } else {
                            ServiceControlHandlerResult::NoError
                        }
                    }
                    ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                    _ => ServiceControlHandlerResult::NotImplemented,
                }
            })
            .map_err(|e| anyhow!("Failed to register service: {:?}", e))?,
        );

        let main = Main::init()?;

        lock.unwrap()
            .set_service_status(ServiceStatus {
                controls_accepted: ServiceControlAccept::SHUTDOWN | ServiceControlAccept::STOP,
                current_state: windows_service::service::ServiceState::Running,
                service_type: ServiceType::OWN_PROCESS,
                exit_code: ServiceExitCode::NO_ERROR,
                checkpoint: 0,
                process_id: Some(std::process::id()),
                wait_hint: Duration::ZERO,
            })
            .map_err(|e| anyhow!("Failed to set service to running: {:?}", e))?;
        main
    };

    let result = main.run(stop_rx);
    let lock = status_handle.lock().unwrap();
    let code = if let Err(error) = result {
        error!("Service died: {:?}", error);
        ServiceExitCode::ServiceSpecific(1)
    } else {
        ServiceExitCode::NO_ERROR
    };
    lock.unwrap()
        .set_service_status(ServiceStatus {
            controls_accepted: ServiceControlAccept::empty(),
            current_state: windows_service::service::ServiceState::Stopped,
            service_type: ServiceType::OWN_PROCESS,
            exit_code: code,
            checkpoint: 0,
            process_id: Some(std::process::id()),
            wait_hint: Duration::ZERO,
        })
        .map_err(|e| anyhow!("Failed to set service to stopped: {:?}", e))?;

    Ok(())
}

#[cfg(windows)]
pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    use windows_service::{
        service::{ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType},
        service_dispatcher,
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    match std::env::args().nth(1).as_deref() {
        Some("service-register") => {
            let manager =
                ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;
            manager.create_service(
                &ServiceInfo {
                    name: "laing-controller".into(),
                    display_name: "Laing Controller".into(),
                    service_type: ServiceType::OWN_PROCESS,
                    start_type: ServiceStartType::AutoStart,
                    error_control: ServiceErrorControl::Normal,
                    executable_path: std::env::current_exe()?,
                    launch_arguments: vec!["service".into()],
                    dependencies: vec![],
                    account_name: None,
                    account_password: None,
                },
                ServiceAccess::QUERY_STATUS,
            )?;
            Ok(())
        }
        Some("service-deregister") => {
            let manager =
                ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;
            let service = manager.open_service("laing-controller", ServiceAccess::DELETE)?;
            service.delete()?;
            Ok(())
        }
        Some("log-register") => {
            eventlog::register("laing-controller")?;
            Ok(())
        }
        Some("log-deregister") => {
            eventlog::deregister("laing-controller")?;
            Ok(())
        }
        Some("service") => {
            let level = match std::env::var("LC_LOG_LEVEL").ok().as_deref() {
                Some("trace") => log::Level::Trace,
                Some("debug") => log::Level::Debug,
                Some("info") => log::Level::Info,
                Some("warn") => log::Level::Warn,
                Some("error") => log::Level::Error,
                _ => log::Level::Info,
            };
            eventlog::init("laing-controller", level).unwrap();

            service_dispatcher::start("laing-controller", ffi_service_main)?;
            Ok(())
        }
        Some(other) => return Err(anyhow!("Unexpected parameter: {}", other).into()),
        _ => {
            standard_main()?;
            Ok(())
        }
    }
}

#[cfg(not(windows))]
pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    standard_main()?;
    Ok(())
}

pub fn standard_main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let (stop_tx, stop_rx) = oneshot::channel();
    Main::init()?.run(stop_rx)?;
    std::mem::drop(stop_tx);
    Ok(())
}

struct Main {
    settings: Settings,
    mqtt: MqttHandle,
    state: State,
}

impl Main {
    pub fn init() -> anyhow::Result<Main> {
        let settings = load_settings()?;

        let (height_send, height_receive) = tokio::sync::watch::channel(None);
        let (command_send, command_receive) = tokio::sync::broadcast::channel(2);

        let mqtt = MqttHandle {
            height: height_send,
            command: command_receive,
        };

        let state = State {
            height: height_receive,
            command: command_send,
        };

        Ok(Main {
            settings,
            mqtt,
            state,
        })
    }

    #[tokio::main(flavor = "current_thread")]
    pub async fn run(self, stop: oneshot::Receiver<()>) -> anyhow::Result<()> {
        let port = TransferPort::new(TimeoutPort::new(
            SerialStream::open(
                &tokio_serial::new(&self.settings.serial_port, 57600)
                    .timeout(Duration::from_millis(250)),
            )?,
            Duration::from_millis(500),
        ));

        tokio::select! {
            result = main_loop(port, self.mqtt, stop) => result?,
            result = mqtt_loop(&self.settings, self.state) => result?,
        }

        Ok(())
    }
}

async fn main_loop<T: AsyncRead + AsyncWrite + Send + 'static>(
    mut port: TransferPort<T>,
    mut mqtt: MqttHandle,
    mut stop: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let server_addr = Slave(0x01);
    operate(&mut port, server_addr, None, &mut mqtt).await?;
    info!("Controller initialized");

    loop {
        let command = tokio::select! {
            command = mqtt.command.recv() => command?,
            _ = &mut stop => return Ok(()),
        };
        info!("Got command {:?}", command);
        let command = match command {
            mqtt::Command::Preset1 => Some(&PRESET1),
            mqtt::Command::Preset2 => Some(&PRESET2),
            mqtt::Command::Preset3 => Some(&PRESET3),
            mqtt::Command::Preset4 => Some(&PRESET4),
            mqtt::Command::Refresh => None,
        };
        operate(&mut port, server_addr, command, &mut mqtt).await?;
    }
}
