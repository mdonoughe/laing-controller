use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use log::{error, info};
use rumqttc::{
    AsyncClient, ConnAck, Event, LastWill, MqttOptions, Outgoing, Packet, Publish, QoS,
    TlsConfiguration, Transport,
};

use crate::settings::{MqttTransport, Settings};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Preset1,
    Preset2,
    Preset3,
    Preset4,
    Refresh,
}

pub struct MqttHandle {
    pub height: tokio::sync::watch::Sender<Option<f32>>,
    pub command: tokio::sync::broadcast::Receiver<Command>,
}

impl MqttHandle {
    pub fn set_height(&mut self, height: f32) -> Result<()> {
        self.height
            .send(Some(height))
            .map_err(|_| anyhow!("Failed to send message"))
    }
}

#[derive(Clone)]
pub struct State {
    pub height: tokio::sync::watch::Receiver<Option<f32>>,
    pub command: tokio::sync::broadcast::Sender<Command>,
}

pub async fn mqtt_loop(settings: &Settings, mut state: State) -> Result<()> {
    let connected_topic = format!("{}/{}/connected", settings.prefix, settings.id);
    let height_topic = format!("{}/{}/height", settings.prefix, settings.id);
    let command_topic = format!("{}/{}/command", settings.prefix, settings.id);

    let port = settings
        .mqtt
        .port
        .unwrap_or_else(|| match settings.mqtt.transport {
            MqttTransport::Tcp => 1883,
            MqttTransport::Tls => 8883,
        });
    let mut mqtt_options = MqttOptions::new(&settings.id, &settings.mqtt.host, port);
    match settings.mqtt.transport {
        MqttTransport::Tcp => mqtt_options.set_transport(Transport::Tcp),
        MqttTransport::Tls => {
            let mut config = rumqttc::ClientConfig::new();
            for cert in rustls_native_certs::load_native_certs()? {
                config.root_store.add(&rustls::Certificate(cert.0))?;
            }
            mqtt_options.set_transport(Transport::Tls(TlsConfiguration::Rustls(Arc::new(config))))
        }
    };
    if let Some(credentials) = &settings.mqtt.credentials {
        mqtt_options.set_credentials(&credentials.username, &credentials.password);
    }
    mqtt_options.set_last_will(LastWill::new(
        &connected_topic,
        "OFF",
        QoS::AtLeastOnce,
        true,
    ));

    // Set capacity to 1.
    // Backpressure is handled more intelligently and for this application it just
    // doesn't make sense to buffer multiple values for the same topic.
    let (client, mut event_loop) = AsyncClient::new(mqtt_options, 1);

    let (connect_send, mut connect_receive) = tokio::sync::mpsc::channel(1);
    let command_topic_listen = command_topic.clone();
    let event_loop = tokio::spawn(async move {
        // Keep this separate from the `publish(..).await`s.
        // There's an in-memory queue that holds messages until they are dispatched from
        // this coroutine. If that queue fills up, `publish(..).await` will pause the
        // coroutine until this coroutine makes progress emptying the queue. If they're
        // the same coroutine the code will deadlock as soon as the queue overflows.
        const MIN_DELAY: Duration = Duration::from_secs(1);
        let mut start = Instant::now();
        let mut stop = false;
        loop {
            match event_loop.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(ConnAck {
                    code: rumqttc::ConnectReturnCode::Success,
                    ..
                }))) => {
                    info!("MQTT connected");
                    // LWT sets power to off on disconnect so we need to set power to on
                    // after every connect.
                    // Don't do it from this coroutine or the code can deadlock.
                    let _ = connect_send.try_send(());
                }
                Ok(Event::Outgoing(Outgoing::Disconnect)) => {
                    stop = true;
                }
                Ok(Event::Incoming(Packet::Publish(Publish { topic, payload, .. }))) => {
                    if topic == command_topic_listen {
                        if let Some(preset) = match &payload[..] {
                            b"1" => Some(Command::Preset1),
                            b"2" => Some(Command::Preset2),
                            b"3" => Some(Command::Preset3),
                            b"4" => Some(Command::Preset4),
                            b"REFRESH" => Some(Command::Refresh),
                            _ => None,
                        } {
                            state
                                .command
                                .send(preset)
                                .context("failed to accept command")?;
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    if stop {
                        break;
                    }
                    error!("MQTT error: {:?}", error);

                    // Wait so we don't flood the network with requests and then try again.
                    let elapsed = start.elapsed();
                    if elapsed < MIN_DELAY {
                        tokio::time::sleep(MIN_DELAY - elapsed).await;
                    }
                    start = Instant::now();
                }
            }
        }

        Result::<(), anyhow::Error>::Ok(())
    });

    if !settings.hass_prefix.is_empty() {
        client
            .publish(
                format!(
                    "{}/binary_sensor/{}_connected/config",
                    settings.hass_prefix, settings.id
                ),
                QoS::AtLeastOnce,
                true,
                serde_json::to_string(&serde_json::json!({
                    "name": format!("{} Connected", settings.name),
                    "device_class": "connectivity",
                    "state_topic": &connected_topic,
                }))
                .unwrap(),
            )
            .await?;
        client
            .publish(
                format!(
                    "{}/sensor/{}_height/config",
                    settings.hass_prefix, settings.id
                ),
                QoS::AtLeastOnce,
                true,
                serde_json::to_string(&serde_json::json!({
                    "name": format!("{} Height", settings.name),
                    "unit_of_measurement": "in",
                    "state_topic": &height_topic,
                    "availability": [{
                        "topic": &connected_topic,
                        "payload_available": "ON",
                        "payload_not_available": "OFF",
                    }],
                    "icon": "mdi:human-male-height",
                }))
                .unwrap(),
            )
            .await?;

        for i in 1..=4 {
            client
                .publish(
                    format!(
                        "{}/button/{}_preset_{}/config",
                        settings.hass_prefix, settings.id, i
                    ),
                    QoS::AtLeastOnce,
                    true,
                    serde_json::to_string(&serde_json::json!({
                        "name": format!("{} {}", settings.name, i),
                        "command_topic": &command_topic,
                        "payload_press": format!("{}", i),
                        "availability": [{
                            "topic": &connected_topic,
                            "payload_available": "ON",
                            "payload_not_available": "OFF",
                        }],
                        "icon": format!("mdi:numeric-{}-circle", i),
                    }))
                    .unwrap(),
                )
                .await?;
        }
        client
            .publish(
                format!(
                    "{}/button/{}_refresh/config",
                    settings.hass_prefix, settings.id
                ),
                QoS::AtLeastOnce,
                true,
                serde_json::to_string(&serde_json::json!({
                    "name": format!("{} refresh", settings.name),
                    "command_topic": &command_topic,
                    "payload_press": "REFRESH",
                    "availability": [{
                        "topic": &connected_topic,
                        "payload_available": "ON",
                        "payload_not_available": "OFF",
                    }],
                    "icon": "mdi:refresh",
                }))
                .unwrap(),
            )
            .await?;
    }

    let worker = tokio::spawn(async move {
        loop {
            tokio::select! {
                recv = connect_receive.recv() => {
                    if recv.is_some() {
                        client.subscribe(&command_topic, QoS::AtMostOnce).await?;
                        client.publish(&connected_topic, QoS::AtLeastOnce, true, "ON").await?;
                    } else {
                        break;
                    }
                }
                recv = state.height.changed() => {
                    if recv.is_err() {
                        break;
                    }
                    let height = *state.height.borrow_and_update();
                    if let Some(height) = height {
                        client.publish(&height_topic, QoS::AtLeastOnce, true, format!("{}", height)).await?;
                    }
                }
            }
        }
        client.disconnect().await?;
        Result::<(), anyhow::Error>::Ok(())
    });

    tokio::select! {
        res = worker => res??,
        res = event_loop => res??,
    };

    Ok(())
}
