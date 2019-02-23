#[macro_use]
extern crate clap;
extern crate futures;
extern crate tokio_core;
extern crate tokio_modbus;
extern crate tokio_serial;
extern crate tokio_timer;

mod timeout;
mod transfer;

use clap::{Arg, SubCommand};
use futures::future::{self, Future};
use std::io;
use std::time::Duration;
use timeout::TimeoutPort;
use tokio_core::reactor::{Core, Handle};
use tokio_io::AsyncRead;
use tokio_io::AsyncWrite;
use tokio_modbus::client::Context;
use tokio_modbus::prelude::*;
use tokio_serial::{Serial, SerialPortSettings};
use transfer::TransferPort;

struct Modbus<T> {
    handle: Handle,
    port: TransferPort<T>,
    slave: Slave,
}

impl<T: AsyncRead + AsyncWrite + 'static> Modbus<T> {
    pub fn connect(&self) -> impl Future<Item = Context, Error = io::Error> {
        rtu::connect_slave(&self.handle, self.port.take(), self.slave)
    }
}

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

pub fn main() {
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

    let mut core = Core::new().unwrap();
    let tty_path = matches.value_of("port").unwrap();
    let server_addr = Slave(0x01);

    let mut settings = SerialPortSettings::default();
    settings.baud_rate = 57600;
    settings.timeout = Duration::from_millis(250);
    let port = TransferPort::new(TimeoutPort::new(
        Serial::from_path(tty_path, &settings).unwrap(),
        Duration::from_secs(1),
    ));
    let modbus = Modbus {
        handle: core.handle(),
        port,
        slave: server_addr,
    };

    let command = match matches.subcommand() {
        ("up", _) => UP,
        ("down", _) => DOWN,
        _ => {
            matches.usage();
            std::process::exit(1);
        }
    };

    let task = future::loop_fn(modbus, move |modbus| {
        modbus.connect().and_then(move |client| {
            println!("sending wake message");
            client
                .read_write_multiple_registers(0x9c4, 20, 0xa8c, &WAKE[..])
                .and_then(move |res| {
                    println!("memory value is: {:?}", res);
                    Ok(())
                })
                .then(|res| {
                    Ok(match res {
                        Ok(_) => future::Loop::Break(client),
                        Err(err) => {
                            println!("error: {:?}", err);
                            future::Loop::Continue(modbus)
                        }
                    })
                })
        })
    })
    .and_then(move |client| {
        println!("sending idle");
        client
            .read_write_multiple_registers(0x9c4, 20, 0xa8c, &IDLE[..])
            .and_then(move |res| {
                println!("memory value is: {:?}", res);
                Ok(client)
            })
    })
    .and_then(move |client| {
        println!("sending lead");
        client
            .read_write_multiple_registers(0x9c4, 20, 0xa8c, &command[0][..])
            .and_then(move |res| {
                println!("memory value is: {:?}", res);
                Ok((client, res[7]))
            })
    })
    .and_then(move |(client, last_height)| {
        future::loop_fn(
            (client, last_height, 0),
            move |(client, last_height, since_change)| {
                tokio_timer::sleep(Duration::from_millis(500))
                    .map_err(|_| io::Error::from(io::ErrorKind::Interrupted))
                    .and_then(move |_| {
                        println!("sending command");
                        client
                            .read_write_multiple_registers(0x9c4, 20, 0xa8c, &command[1][..])
                            .and_then(move |res| {
                                println!("memory value is: {:?}", res);
                                if res[7] == last_height {
                                    if since_change < 1 {
                                        Ok(future::Loop::Continue((
                                            client,
                                            res[7],
                                            since_change + 1,
                                        )))
                                    } else {
                                        Ok(future::Loop::Break(client))
                                    }
                                } else {
                                    Ok(future::Loop::Continue((client, res[7], 0)))
                                }
                            })
                    })
            },
        )
    })
    .and_then(move |client| {
        println!("sending idle");
        client
            .read_write_multiple_registers(0x9c4, 20, 0xa8c, &IDLE[..])
            .and_then(move |res| {
                println!("memory value is: {:?}", res);
                Ok(client)
            })
    });

    core.run(task).unwrap();
}
