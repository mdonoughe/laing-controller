# laing-controller

> Connects Laing Innotech motorized desks to automation by MQTT.

laing-controller is a service that connects to a Laing Innotech desk motor controller using modbus over RS485. This is PC software intended to be run from a computer attached to the desk. You'll need hardware for this.

## Hardware

Follow these instructions at your own risk. Check with a multimeter that the voltages make sense before connecting your expensive desk to your expensive computer. Take extra care to ensure that unexpected movement of the desk will not hit you or anything else when wiring this up.

I recommend getting RJ25 (AKA 6P6C) connectors and a short RJ25 cable, rather than cutting and splicing cable the controller came with. To begin with, connect the pins straight through. It may be possible to do this by buying a barrel connector, but I did it using some cheap breakout boards I got on Amazon.

You will also need a way for your computer to speak RS485. I use an FTDI USB-RS485.

RJ25 is similar to the RJ11 typically used for old phones, but all six conductors are present. The controller only uses four pins, similar to RJ14, but the pins used are 1 2 3 6, not 2 3 4 5 as used for phones.

Do not just buy a USB RJ45 RS485 adapter and assume the wiring is correct. There does not seem to be a standard way to wire these connectors. While writing this documentation, I found only incompatible wiring diagrams.

Pins 1 and 6 are power and ground. Connect the ground wire from your computer to the ground wire of the controller. You can leave the power wire from your computer disconnected because the controller is already providing power.

Pins 2 and 3 are the differential data pair plus and minus wires.

If you get an adapter which has a terminator pair, you can leave that not connected because you're tapping into a connection that is already terminated.

## Warning

This can be dangerous. Do not put things where they can get in the way of the desk moving. Do not operate the desk unattended. Consider the security aspects of making motorized equipment accessible over the network.

## Compatible controllers

The only controller confirmed to be compatible is the LTC302-US-00-00-G0.

## Configuration and topics

See the file laing-controller.yaml.

## Home Assistant

If you are using [Home Assistant] and have [MQTT discovery] enabled (enabled by default when you configure MQTT), entities will be automatically created within Home Assistant.

- binary_sensor.NAME_connected - ON when the program is running and connected to MQTT
- button.NAME_1 - press to go to preset 1
- button.NAME_2 - press to go to preset 2
- button.NAME_3 - press to go to preset 3
- button.NAME_4 - press to go to preset 4
- button.NAME_refresh - press to refresh the height (useful if the physical buttons have been used)
- sensor.NAME_height - the current height of the desk (in inches)

Where NAME is replaced by the name specified in the configuration file.

You can use the following card configuration to make it appear in Lovelace:

![Preview of example Lovelace configuration](lovelace.png)

```yaml
type: buttons
entities:
  - entity: button.NAME_1
    tap_action:
      action: call-service
      service: button.press
      service_data:
        entity_id: button.NAME_1
  - entity: button.NAME_2
    tap_action:
      action: call-service
      service: button.press
      service_data:
        entity_id: button.NAME_2
  - entity: button.NAME_3
    tap_action:
      action: call-service
      service: button.press
      service_data:
        entity_id: button.NAME_3
  - entity: button.NAME_4
    tap_action:
      action: call-service
      service: button.press
      service_data:
        entity_id: button.NAME_4
  - entity: button.NAME_refresh
    tap_action:
      action: call-service
      service: button.press
      service_data:
        entity_id: button.NAME_refresh
```

[Home Assistant]: https://www.home-assistant.io/
[MQTT discovery]: https://www.home-assistant.io/docs/mqtt/discovery/

## Known issues

laing-controller does not snoop the modbus connection, so when you adjust the hight of the desk using the controls that the desk came with, laing-controller does not notice and does not report the new height over MQTT. You can request a refresh to fix this.

Due to problems with tokio-modbus and tokio-serial, laing-controller stays connected to the serial port, despite not snooping the connection. As a consequence, when the desk height is changed using the controls that the desk came with, the chatter between the controller and its button panel accumulates in laing-controller's read buffer, so when it tries to communicate to the controller it reads garbage data. tokio-modbus skips over some of this, but generally the first few transactions will fail. You can avoid this by requesting a few refreshes before trying to move the desk.

I'm not sure if these problems are worth the effort of implementing snooping.
