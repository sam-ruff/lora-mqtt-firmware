//! Hub control task: provisioning and status over the host link.
//!
//! Consumes [`HubCommandEnvelope`]s, applies them to a pending copy of the
//! configuration, persists changes to flash and replies with pre-encoded
//! hub-protocol frames. Settings apply on the next boot (the host sends the
//! stock Reboot command after provisioning).

use embassy_futures::select::{select, Either};
use embassy_time::Instant;
use hub_protocol::{AckStatus, HubCommand, HubResponse};

use crate::debug;
use crate::dispatcher::{
    ResponseMessage, HUB_CHANNEL, PORTAL_REPLY, PORTAL_REQUEST, RESPONSE_CHANNEL,
};
use crate::hub_config::store::ConfigStore;
use crate::hub_config::{apply_command, network_config_response, ConfigAction, HubConfig};
use crate::net::stats::HUB_STATS;

pub async fn hub_ctrl_task(mut store: Option<ConfigStore>, active: &'static HubConfig) {
    let receiver = HUB_CHANNEL.receiver();
    let portal_receiver = PORTAL_REQUEST.receiver();
    let response_pub = RESPONSE_CHANNEL.immediate_publisher();
    // Working copy: starts as the active config, tracks sets before reboot.
    let mut pending = active.clone();

    loop {
        match select(receiver.receive(), portal_receiver.receive()).await {
            Either::First(envelope) => {
                let response = handle(&mut store, &mut pending, &envelope.command).await;
                let frame = hub_protocol::encode_response(&response);
                response_pub.publish_immediate(ResponseMessage::HubRaw {
                    source: envelope.source,
                    frame,
                });
            }
            Either::Second(command) => {
                let response = handle(&mut store, &mut pending, &command).await;
                PORTAL_REPLY.send(response).await;
            }
        }
    }
}

async fn handle(
    store: &mut Option<ConfigStore>,
    pending: &mut HubConfig,
    command: &HubCommand,
) -> HubResponse {
    match command {
        HubCommand::GetNetworkConfig => network_config_response(pending),
        HubCommand::GetHubStatus => {
            let uptime_secs = Instant::now().as_secs() as u32;
            HubResponse::HubStatus(HUB_STATS.snapshot(uptime_secs))
        }
        command => {
            let status = match apply_command(pending, command) {
                Ok(action) => persist(store, pending, action).await,
                Err(status) => status,
            };
            HubResponse::ConfigAck { status }
        }
    }
}

async fn persist(
    store: &mut Option<ConfigStore>,
    pending: &HubConfig,
    action: ConfigAction,
) -> AckStatus {
    let Some(store) = store.as_mut() else {
        debug!("Hub ctrl: no config store, setting not persisted");
        return AckStatus::StorageError;
    };
    let result = match action {
        ConfigAction::Save => store.save(pending).await,
        ConfigAction::Clear => store.clear().await,
        ConfigAction::None => Ok(()),
    };
    match result {
        Ok(()) => AckStatus::Ok,
        Err(err) => {
            debug!("Hub ctrl: flash store failed: {:?}", err);
            AckStatus::StorageError
        }
    }
}
