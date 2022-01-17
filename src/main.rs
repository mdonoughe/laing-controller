#[macro_use]
extern crate clap;

mod timeout;
mod transfer;

use clap::{Arg, SubCommand};
use tokio_serial::{SerialStream};
use std::time::Duration;
use timeout::TimeoutPort;
use tokio_modbus::prelude::*;
use transfer::TransferPort;

static WAKE: [u16; 14] = [
    0x0000, 0x0000, 0x0009, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017, 0x0000,
    0x0000, 0x0000,
];
static IDLE: [u16; 14] = [
    0x0000, 0x0000, 0x0000, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017, 0x0000,
    0x0000, 0x0000,
];
static UP: [[u16; 14]; 2] = [
    [
        0x0000, 0x0000, 0x0001, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
    [
        0x0000, 0x0000, 0x0001, 0x0001, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
];
static DOWN: [[u16; 14]; 2] = [
    [
        0x0000, 0x0000, 0x0002, 0x0000, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
    [
        0x0000, 0x0000, 0x0002, 0x0002, 0x0008, 0x0005, 0x0001, 0x005A, 0x0011, 0x0008, 0x0017,
        0x0000, 0x0000, 0x0000,
    ],
];

#[tokio::main(flavor = "current_thread")]
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let matches = app_from_crate!()
        .arg(
            Arg::with_name("port")
                .short("p")
                .value_name("PORT")
                .default_value("COM1")
                .help("The serial port the desk is connected to"),
        )
        .subcommand(SubCommand::with_name("up"))
        .subcommand(SubCommand::with_name("down"))
        .get_matches();

    let tty_path = matches.value_of("port").unwrap();
    let server_addr = Slave(0x01);

    let settings = tokio_serial::new(tty_path, 57600)
        .timeout(Duration::from_millis(250));

    let command = match matches.subcommand() {
        ("up", _) => UP,
        ("down", _) => DOWN,
        _ => {
            matches.usage();
            std::process::exit(1);
        }
    };

    let port = TransferPort::new(TimeoutPort::new(SerialStream::open(&settings)?, Duration::from_millis(500)));
    let mut client = rtu::connect_slave(port.take(), server_addr).await?;
    println!("sending wake message");
    loop {
        match client.read_write_multiple_registers(0x9c4, 20, 0xa8c, &WAKE[..]).await {
            Ok(v) => {
                println!("memory value is: {:?}", v);
                break;
            }
            Err(err) => {
                println!("error: {:?}", err);
                client.disconnect().await?;
                client = rtu::connect_slave(port.take(), server_addr).await?;
            }
        }
    }
    println!("sending idle");
    let res = client
        .read_write_multiple_registers(0x9c4, 20, 0xa8c, &IDLE[..]).await?;
    println!("memory value is: {:?}", res);
    println!("sending lead");
    let res = client
        .read_write_multiple_registers(0x9c4, 20, 0xa8c, &command[0][..]).await?;
    println!("memory value is: {:?}", res);
    let mut last_height = res[7];
    let mut since_change = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        println!("sending command");
        let res = client
            .read_write_multiple_registers(0x9c4, 20, 0xa8c, &command[1][..]).await?;
        println!("memory value is: {:?}", res);
        if res[7] == last_height {
            if since_change < 1 {
                since_change += 1;
            } else {
                break;
            }
        } else {
            last_height = res[7];
        }
    }
    println!("sending idle");
    let res = client
        .read_write_multiple_registers(0x9c4, 20, 0xa8c, &IDLE[..]).await?;
    println!("memory value is: {:?}", res);

    client.disconnect().await?;
    Ok(())
}
