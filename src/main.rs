mod mqtt;
mod settings;
mod timeout;
mod transfer;

use log::{debug, error, info};
use mqtt::{MqttHandle, State};
use settings::load_settings;
use std::time::Duration;
use timeout::TimeoutPort;
use tokio::io::{AsyncRead, AsyncWrite};
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

#[tokio::main(flavor = "current_thread")]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let settings = load_settings().await?;

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

    let port = TransferPort::new(TimeoutPort::new(
        SerialStream::open(
            &tokio_serial::new(&settings.serial_port, 57600).timeout(Duration::from_millis(250)),
        )?,
        Duration::from_millis(500),
    ));

    tokio::select! {
        result = main_loop(port, mqtt) => result?,
        result = mqtt_loop(&settings, state) => result?,
    }

    Ok(())
}

async fn main_loop<T: AsyncRead + AsyncWrite + Send + 'static>(
    mut port: TransferPort<T>,
    mut mqtt: MqttHandle,
) -> anyhow::Result<()> {
    let server_addr = Slave(0x01);
    operate(&mut port, server_addr, None, &mut mqtt).await?;
    info!("Controller initialized");

    loop {
        let command = mqtt.command.recv().await?;
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
